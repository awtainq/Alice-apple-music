use anyhow::{anyhow, bail, ensure, Context, Result};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self},
    path::{Path, PathBuf},
    process::Command,
};
use tar::{Builder, Header};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
fn version(repo: &Path) -> Result<String> {
    Ok(fs::read_to_string(repo.join("VERSION"))?.trim().to_owned())
}
fn append<W: io::Write>(tar: &mut Builder<W>, name: &str, data: &[u8], mode: u32) -> Result<()> {
    let mut h = Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(mode);
    h.set_mtime(0);
    h.set_uid(0);
    h.set_gid(0);
    h.set_cksum();
    tar.append_data(&mut h, name, data)?;
    Ok(())
}
fn archive(path: &Path) -> Result<Builder<GzEncoder<fs::File>>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(Builder::new(
        GzBuilder::new()
            .mtime(0)
            .write(fs::File::create(path)?, Compression::default()),
    ))
}
fn finish(tar: Builder<GzEncoder<fs::File>>, output: &Path) -> Result<()> {
    tar.into_inner()?.finish()?;
    let digest = hex::encode(Sha256::digest(fs::read(output)?));
    fs::write(
        format!("{}.sha256", output.display()),
        format!(
            "{digest}  {}\n",
            output.file_name().unwrap().to_string_lossy()
        ),
    )?;
    println!("{}\nSHA256 {digest}", output.display());
    Ok(())
}
fn validate_binary(data: &[u8], forbidden: &[&str]) -> Result<()> {
    ensure!(
        data.len() >= 20
            && &data[..4] == b"\x7fELF"
            && data[4] == 2
            && u16::from_le_bytes([data[18], data[19]]) == 183,
        "Expected an ARM64 ELF executable"
    );
    for prefix in forbidden {
        if !prefix.is_empty() && data.windows(prefix.len()).any(|b| b == prefix.as_bytes()) {
            bail!("Personal build path found; rebuild with rust/build.sh");
        }
    }
    Ok(())
}
fn package(repo: &Path, binary: &Path, output: &Path) -> Result<()> {
    let data = fs::read(binary).context("Read release binary")?;
    let home = env::var("HOME").unwrap_or_default();
    let dir = repo.to_string_lossy();
    validate_binary(&data, &[&home, &dir])?;
    let manifest = fs::read_to_string(repo.join("rust/luna/plugin.toml"))?;
    ensure!(
        manifest
            .lines()
            .any(|l| l == format!("version = \"{}\"", version(repo).unwrap_or_default())),
        "Manifest and VERSION differ"
    );
    let mut tar = archive(output)?;
    append(&mut tar, "plugin.toml", manifest.as_bytes(), 0o644)?;
    append(&mut tar, "bin/alice-apple-music", &data, 0o755)?;
    for (name, file) in [
        ("README.md", "rust/README.md"),
        ("LICENSE", "rust/LICENSE"),
        ("NOTICE", "rust/NOTICE"),
    ] {
        append(&mut tar, name, &fs::read(repo.join(file))?, 0o644)?;
    }
    finish(tar, output)
}
fn add_tree<W: io::Write>(
    tar: &mut Builder<W>,
    path: &Path,
    base: &Path,
    prefix: &str,
) -> Result<()> {
    let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        ensure!(!kind.is_symlink(), "Unexpected symlink in source tree");
        if kind.is_dir() {
            add_tree(tar, &entry.path(), base, prefix)?;
        } else if kind.is_file() {
            let name = format!(
                "{prefix}/{}",
                entry.path().strip_prefix(base)?.to_string_lossy()
            );
            append(tar, &name, &fs::read(entry.path())?, 0o644)?;
        }
    }
    Ok(())
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn source(repo: &Path, output: &Path) -> Result<()> {
    let temp = Temp(env::temp_dir().join(format!("alice-music-source-{}", std::process::id())));
    fs::create_dir(&temp.0)?;
    let vendor = temp.0.join("vendor");
    let status = Command::new("cargo")
        .args(["vendor", "--locked", "--manifest-path"])
        .arg(repo.join("rust/Cargo.toml"))
        .arg(&vendor)
        .stdout(std::process::Stdio::null())
        .status()?;
    ensure!(status.success(), "Cargo vendor failed");
    let prefix = format!("alice-apple-music-{}-source", version(repo)?);
    let mut tar = archive(output)?;
    // Only tracked files enter the project source bundle; personal files stay out.
    let tracked = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-z"])
        .output()?;
    ensure!(tracked.status.success(), "git ls-files failed");
    for raw in tracked.stdout.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let rel = std::str::from_utf8(raw)?;
        let path = repo.join(rel);
        if !path.is_file() {
            continue;
        }
        append(
            &mut tar,
            &format!("{prefix}/{rel}"),
            &fs::read(path)?,
            if rel.ends_with(".sh") { 0o755 } else { 0o644 },
        )?;
    }
    add_tree(&mut tar, &vendor, &temp.0, &prefix)?;
    append(&mut tar,&format!("{prefix}/.cargo/config.toml"),b"[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"vendor\"\n",0o644)?;
    finish(tar, output)
}
fn run() -> Result<()> {
    let repo = root();
    let mut args = env::args().skip(1);
    let task = args.next().unwrap_or_default();
    let mut binary = repo.join("rust/target/aarch64-unknown-linux-musl/release/alice-apple-music");
    let mut output = None;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| anyhow!("Missing option value"))?;
        match arg.as_str() {
            "--binary" => binary = PathBuf::from(value),
            "--output" => output = Some(PathBuf::from(value)),
            _ => bail!("Unknown option"),
        }
    }
    match task.as_str(){
        "package"=>package(&repo,&binary,&output.unwrap_or(repo.join(format!("dist/alice-apple-music-luna-{}-aarch64.yap",version(&repo)?)))),
        "source"=>source(&repo,&output.unwrap_or(repo.join(format!("dist/alice-apple-music-{}-source.tar.gz",version(&repo)?)))),
        _=>bail!("Usage: cargo run --manifest-path rust/Cargo.toml -p xtask -- package|source [--output PATH] [--binary PATH]")
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    fn binary() -> Vec<u8> {
        let mut b = vec![0; 20];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[18..20].copy_from_slice(&183u16.to_le_bytes());
        b
    }
    #[test]
    fn rejects_other_architectures() {
        assert!(validate_binary(b"not an ELF", &[]).is_err());
        let mut b = binary();
        b[18] = 62;
        assert!(validate_binary(&b, &[]).is_err());
    }
    #[test]
    fn rejects_personal_paths() {
        let mut b = binary();
        b.extend(b"/private/build/home");
        assert!(validate_binary(&b, &["/private/build"]).is_err());
        assert!(validate_binary(&binary(), &["/private/build"]).is_ok());
    }
    #[test]
    fn package_is_reproducible_and_has_only_runtime_files() {
        let temp = Temp(env::temp_dir().join(format!("alice-package-test-{}", std::process::id())));
        fs::create_dir(&temp.0).unwrap();
        let executable = temp.0.join("player");
        fs::write(&executable, binary()).unwrap();
        let first = temp.0.join("first.yap");
        let second = temp.0.join("second.yap");
        package(&root(), &executable, &first).unwrap();
        package(&root(), &executable, &second).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        let gzip = flate2::read::GzDecoder::new(fs::File::open(&first).unwrap());
        let mut archive = tar::Archive::new(gzip);
        let names: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().into_owned())
            .collect();
        let expected: Vec<PathBuf> = [
            "plugin.toml",
            "bin/alice-apple-music",
            "README.md",
            "LICENSE",
            "NOTICE",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(names, expected);
    }
    #[test]
    fn deterministic_archive_metadata_and_contents() {
        let mut tar = Builder::new(Vec::new());
        append(&mut tar, "bin/player", b"test", 0o755).unwrap();
        let raw = tar.into_inner().unwrap();
        let mut archive = tar::Archive::new(&raw[..]);
        let mut entries = archive.entries().unwrap();
        let mut file = entries.next().unwrap().unwrap();
        assert_eq!(file.header().mtime().unwrap(), 0);
        assert_eq!(file.header().uid().unwrap(), 0);
        assert_eq!(file.header().mode().unwrap(), 0o755);
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "test");
    }
}

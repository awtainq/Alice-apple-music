use anyhow::{anyhow, bail, ensure, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
pub const MAX_FRAME: usize = 600_000;
pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn varint(mut n: u64) -> Vec<u8> {
    let mut o = vec![];
    while n >= 128 {
        o.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    o.push(n as u8);
    o
}
pub fn number(field: u64, value: u64) -> Vec<u8> {
    let mut o = varint(field << 3);
    o.extend(varint(value));
    o
}
pub fn bytes(field: u64, value: &[u8]) -> Vec<u8> {
    let mut o = varint(field << 3 | 2);
    o.extend(varint(value.len() as u64));
    o.extend(value);
    o
}
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Number(u64),
    Bytes(Vec<u8>),
}
pub fn fields(data: &[u8]) -> Result<Vec<(u64, Field)>> {
    fn take(data: &[u8], p: &mut usize) -> Result<u64> {
        let mut n = 0;
        for shift in (0..70).step_by(7) {
            let b = *data.get(*p).ok_or_else(|| anyhow!("Truncated varint"))?;
            *p += 1;
            if shift == 63 && b > 1 {
                bail!("Oversized varint");
            }
            n |= ((b & 127) as u64) << shift;
            if b < 128 {
                return Ok(n);
            }
        }
        bail!("Invalid varint")
    }
    let mut out = vec![];
    let mut p = 0;
    while p < data.len() {
        let tag = take(data, &mut p)?;
        ensure!(tag >> 3 > 0, "Invalid field");
        let v = match tag & 7 {
            0 => Field::Number(take(data, &mut p)?),
            1 | 2 | 5 => {
                let n = match tag & 7 {
                    1 => 8,
                    5 => 4,
                    _ => usize::try_from(take(data, &mut p)?)?,
                };
                ensure!(n <= data.len() - p, "Truncated field");
                let b = data[p..p + n].to_vec();
                p += n;
                Field::Bytes(b)
            }
            _ => bail!("Unsupported wire type"),
        };
        out.push((tag >> 3, v));
    }
    Ok(out)
}
pub fn get_bytes(data: &[u8], field: u64) -> Result<Option<Vec<u8>>> {
    Ok(fields(data)?.into_iter().find_map(|(n, v)| {
        if n == field {
            if let Field::Bytes(b) = v {
                return Some(b);
            }
        }
        None
    }))
}
pub fn get_number(data: &[u8], field: u64) -> Result<Option<u64>> {
    Ok(fields(data)?.into_iter().find_map(|(n, v)| {
        if n == field {
            if let Field::Number(b) = v {
                return Some(b);
            }
        }
        None
    }))
}
pub fn recognition(frame: &[u8]) -> Result<Option<(u64, String)>> {
    if get_number(frame, 253)? != Some(3) {
        return Ok(None);
    }
    let Some(body) = get_bytes(frame, 252)? else {
        return Ok(None);
    };
    let state = get_number(&body, 1)?.unwrap_or(0);
    let text = if state == 3 {
        String::from_utf8(get_bytes(&body, 2)?.unwrap_or_default())?
    } else {
        String::new()
    };
    Ok(Some((state, text)))
}
pub fn rpc(field: u64, payload: &[u8]) -> Vec<u8> {
    let mut remote = bytes(7, b"AliceCapability");
    remote.extend(bytes(field, payload));
    let mut envelope = bytes(252, &remote);
    envelope.extend(number(253, 210));
    let mut out = (envelope.len() as u32).to_le_bytes().to_vec();
    out.extend(envelope);
    out
}
pub fn cancel() -> Vec<u8> {
    [3, 6, 14]
        .into_iter()
        .flat_map(|n| rpc(8, &[8, n, 56, 1]))
        .collect()
}
pub async fn read_frame(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let size = socket.read_u32_le().await? as usize;
    ensure!(size > 0 && size <= MAX_FRAME, "Invalid IPC frame size");
    let mut b = vec![0; size];
    socket.read_exact(&mut b).await?;
    Ok(b)
}
pub async fn cancel_dialog(port: u16) -> Result<()> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
    s.write_all(&cancel()).await?;
    Ok(())
}
pub async fn speak(port: u16, text: &str) -> Result<()> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await?;
    let sub = rpc(20, &bytes(1, b"AliceCapability"));
    let mut directive = bytes(1, text.as_bytes());
    directive.extend(number(2, 2));
    let mut payload = number(1, 18);
    payload.extend(number(3, 1));
    payload.extend(bytes(13, &directive));
    socket.write_all(&sub).await?;
    socket.write_all(&rpc(8, &payload)).await?;
    let _ = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if read_frame(&mut socket).await.is_err() {
                break;
            }
        }
    })
    .await;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_and_overflow() {
        for b in [vec![255; 20], vec![0], vec![10, 127]] {
            assert!(fields(&b).is_err());
        }
    }
    #[test]
    fn final_only() {
        let mut body = number(1, 3);
        body.extend(bytes(2, "дальше".as_bytes()));
        let mut frame = bytes(252, &body);
        frame.extend(number(253, 3));
        assert_eq!(recognition(&frame).unwrap(), Some((3, "дальше".into())));
    }
    #[test]
    fn max_varint() {
        let v = varint(u64::MAX);
        let mut d = number(1, u64::MAX);
        assert_eq!(v.len(), 10);
        assert_eq!(fields(&d).unwrap(), vec![(1, Field::Number(u64::MAX))]);
        d.push(0);
        assert!(fields(&d).is_err());
    }
}

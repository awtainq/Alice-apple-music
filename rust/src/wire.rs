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
    let phase = get_number(&body, 1)?;
    if phase == Some(3) {
        let text = String::from_utf8(get_bytes(&body, 2)?.unwrap_or_default())?;
        if !text.is_empty() {
            return Ok(Some((3, text)));
        }
    }
    if matches!(phase, None | Some(3)) {
        if let Some(asr) = get_bytes(&body, 15)? {
            if get_number(&asr, 2)? == Some(1) {
                let text = String::from_utf8(get_bytes(&asr, 1)?.unwrap_or_default())?;
                if !text.is_empty() {
                    return Ok(Some((3, text)));
                }
            }
        }
    }
    let state = phase.unwrap_or(0);
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
    [1, 6, 14, 3]
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
    // Keep the IPC session alive while Quasar dispatches cancellation. Closing
    // immediately can discard work queued by the SDK connection handler.
    let _ = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            read_frame(&mut s).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    })
    .await;
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
    let _ = tokio::time::timeout(Duration::from_secs(65), async {
        let mut started = false;
        loop {
            let Ok(frame) = read_frame(&mut socket).await else {
                break;
            };
            if get_number(&frame, 253).ok().flatten() != Some(3) {
                continue;
            }
            let Some(body) = get_bytes(&frame, 252).ok().flatten() else {
                continue;
            };
            let Some(event) = get_bytes(&body, 18).ok().flatten() else {
                continue;
            };
            match get_number(&event, 1).ok().flatten() {
                Some(6) => started = true,
                Some(1) if started => break,
                _ => {}
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

#[cfg(test)]
mod asr_tests {
    use super::*;
    #[test]
    fn command_origin_final_asr_is_recognized() {
        let mut asr = bytes(1, "играй".as_bytes());
        asr.extend(number(2, 1));
        let mut body = number(1, 3);
        body.extend(bytes(15, &asr));
        let mut frame = bytes(252, &body);
        frame.extend(number(253, 3));
        assert_eq!(recognition(&frame).unwrap(), Some((3, "играй".into())));
    }
    #[test]
    fn idle_snapshot_does_not_replay_old_asr() {
        let mut asr = bytes(1, "играй".as_bytes());
        asr.extend(number(2, 1));
        let mut body = number(1, 0);
        body.extend(bytes(15, &asr));
        let mut frame = bytes(252, &body);
        frame.extend(number(253, 3));
        assert_eq!(recognition(&frame).unwrap(), Some((0, String::new())));
    }
}

#[cfg(test)]
mod replay_regression_tests {
    use super::*;
    #[test]
    fn listening_snapshot_never_replays_previous_final() {
        let asr = [bytes(1, "включи песню Babydoll".as_bytes()), number(2, 1)].concat();
        let body = [number(1, 1), bytes(15, &asr)].concat();
        let frame = [bytes(252, &body), number(253, 3)].concat();
        assert_eq!(recognition(&frame).unwrap(), Some((1, String::new())));
    }
    #[test]
    fn fresh_transcript_wins_over_retained_asr() {
        let asr = [bytes(1, "старая песня".as_bytes()), number(2, 1)].concat();
        let body = [
            number(1, 3),
            bytes(2, "новая песня".as_bytes()),
            bytes(15, &asr),
        ]
        .concat();
        let frame = [bytes(252, &body), number(253, 3)].concat();
        assert_eq!(
            recognition(&frame).unwrap(),
            Some((3, "новая песня".into()))
        );
    }
    #[tokio::test]
    async fn cancellation_keeps_session_open_for_dispatch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let check = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut methods = vec![];
            for _ in 0..4 {
                let frame = read_frame(&mut socket).await.unwrap();
                let remote = get_bytes(&frame, 252).unwrap().unwrap();
                let payload = get_bytes(&remote, 8).unwrap().unwrap();
                methods.push(get_number(&payload, 1).unwrap().unwrap());
            }
            assert_eq!(methods, vec![1, 6, 14, 3]);
            let mut byte = [0];
            assert!(
                tokio::time::timeout(Duration::from_millis(100), socket.read(&mut byte))
                    .await
                    .is_err()
            );
        });
        cancel_dialog(port).await.unwrap();
        check.await.unwrap();
    }
}

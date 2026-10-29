use crate::config::Config;
use anyhow::{anyhow, bail, ensure, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message, Connector};
use uuid::Uuid;
struct Job {
    payload: Value,
    reply: oneshot::Sender<Result<Value>>,
    until: tokio::time::Instant,
}
#[derive(Clone)]
pub struct Station {
    tx: mpsc::Sender<Job>,
    pub state: watch::Receiver<Value>,
}
#[derive(Debug)]
struct LoopbackCertificate;
impl ServerCertVerifier for LoopbackCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            m,
            c,
            s,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            m,
            c,
            s,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl Station {
    pub fn start(c: Arc<Config>, http: reqwest::Client) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job>(16);
        let (state_tx, state) = watch::channel(json!({}));
        tokio::spawn(async move {
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(LoopbackCertificate))
            .with_no_client_auth();
            let tls = Arc::new(tls);
            let mut music_token = c.yandex_music_token.clone();
            loop {
                let session:Result<()> = async {
     let mut token=None;
     for attempt in 0..2{
      let r=http.get("https://quasar.yandex.net/glagol/token").header("Authorization",format!("OAuth {music_token}")).query(&[("device_id",c.station_device_id.as_str()),("platform",c.station_platform.as_str())]).send().await.map_err(|e|anyhow!(e.without_url()))?;
      if r.status().is_success(){token=Some(r.json::<Value>().await?["token"].as_str().ok_or_else(||anyhow!("Missing Station token"))?.to_owned());break;}
      if attempt==0&&(r.status().as_u16()==401||r.status().as_u16()==403){
       let r=http.post("https://oauth.mobile.yandex.net/1/token").form(&[("client_secret","53bc75238f0c4d08a118e51fe9203300"),("client_id","23cabbbdc6cd418abb4b39c32c41195d"),("grant_type","x-token"),("access_token",c.yandex_x_token.as_str())]).send().await?;
       music_token=r.json::<Value>().await?["access_token"].as_str().ok_or_else(||anyhow!("Yandex refresh failed"))?.into();
      }else{bail!("Station authentication failed");}
     }
     let token=token.ok_or_else(||anyhow!("Station token unavailable"))?;
     // Certificate trust exception is confined to this hard-coded loopback endpoint.
     let(url,_)=connect_async_tls_with_config(format!("wss://127.0.0.1:{}",c.station_port),None,false,Some(Connector::Rustls(tls.clone()))).await?;
     let(mut out,mut input)=url.split();let mut waiters:HashMap<String,oneshot::Sender<Result<Value>>>=HashMap::new();
     let envelope=|id:String,payload:Value|json!({"conversationToken":token,"id":id,"payload":payload,"sentTime":crate::wire::epoch_ms()});
     out.send(Message::Text(envelope(Uuid::new_v4().to_string(),json!({"command":"softwareVersion"})).to_string().into())).await?;
     let mut heartbeat=tokio::time::interval(Duration::from_secs(10));
     let result:Result<()> = async {loop{tokio::select!{
      job=rx.recv()=>{let Some(job)=job else{return Ok(())};if job.reply.is_closed()||job.until<tokio::time::Instant::now(){continue;}
 let id=Uuid::new_v4().to_string();let payload=envelope(id.clone(),job.payload);waiters.insert(id,job.reply);out.send(Message::Text(payload.to_string().into())).await?;},
      _=heartbeat.tick()=>{waiters.retain(|_,v|!v.is_closed());out.send(Message::Text(envelope(Uuid::new_v4().to_string(),json!({"command":"softwareVersion"})).to_string().into())).await?;},
      message=input.next()=>{let Some(message)=message else{bail!("Station disconnected")};match message?{
       Message::Text(text)=>{let data:Value=serde_json::from_str(&text)?;if let Some(s)=data.get("state"){let _=state_tx.send(s.clone());}
 if let Some(id)=data["requestId"].as_str(){if let Some(reply)=waiters.remove(id){let r=if data["status"]=="SUCCESS"{Ok(data)}else{Err(anyhow!("Станция отклонила команду."))};let _=reply.send(r);}}},
       Message::Ping(bytes)=>out.send(Message::Pong(bytes)).await?,Message::Close(_)=>bail!("Station closed"),_=>{}}
      }
     }}}.await;
     for(_,reply)in waiters{let _=reply.send(Err(anyhow!("Station disconnected")));}result
    }.await;
                if session.is_err() {
                    eprintln!("station_reconnect");
                    let _ = state_tx.send(json!({}));
                }
                if rx.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
        Self { tx, state }
    }
    pub async fn send(&self, payload: Value) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        let until = tokio::time::Instant::now() + Duration::from_secs(15);
        self.tx
            .send(Job {
                payload,
                reply: tx,
                until,
            })
            .await
            .map_err(|_| anyhow!("Station is stopped"))?;
        tokio::time::timeout_at(until, rx)
            .await
            .map_err(|_| anyhow!("Station timeout"))?
            .map_err(|_| anyhow!("Station disconnected"))?
    }
    pub async fn play(&self, url: &str, track: &crate::apple::Track, id: &str) -> Result<()> {
        ensure!(
            url.starts_with("http://127.0.0.1:"),
            "Audio URL must be local"
        );
        let body=json!({"stream":{"url":url,"format":"MP3","type":"Track","id":id},"metadata":{"title":track.title,"subtitle":track.artist}}).to_string();
        let mut wire = crate::wire::bytes(1, b"audio_play");
        wire.extend(crate::wire::bytes(2, body.as_bytes()));
        self.send(json!({"command":"externalCommandBypass","data":B64.encode(wire)}))
            .await?;
        Ok(())
    }
    pub fn snapshot(&self) -> Value {
        self.state.borrow().clone()
    }
}
#[cfg(test)]
impl Station {
    pub fn fixture() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        let (_state_tx, state) = watch::channel(json!({}));
        Self { tx, state }
    }
}

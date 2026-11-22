pub mod client;
pub mod ingress;
pub mod registration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Public,
    Admin,
}

#[derive(Serialize, Deserialize)]
pub enum Command {
    Register,
    Connect {
        id: String,
        credential: String,
        kind: Kind,
    },
}

async fn read_line<T: AsyncRead + Unpin>(stream: &mut T) -> anyhow::Result<String> {
    let mut data = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        if data.len() >= 8192 {
            anyhow::bail!("relay message too long");
        }
        data.push(byte);
    }
    Ok(String::from_utf8(data)?)
}

async fn write_json<T: AsyncWrite + Unpin, V: Serialize>(
    stream: &mut T,
    value: &V,
) -> anyhow::Result<()> {
    stream.write_all(&serde_json::to_vec(value)?).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

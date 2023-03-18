//! Database deserialization module.

use std::str::FromStr;

use anyhow::{anyhow, Context as _, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::Sql;

/// Token of bencoding.
#[derive(Debug)]
enum BencodeToken {
    /// End "e".
    End,

    /// Length-prefixed bytestring.
    ByteString(Vec<u8>),

    /// Integer like "i1000e".
    Integer(i64),

    /// Beginning of the list "l".
    List,

    /// Beginning of the dictionary "d".
    Dictionary,
}

/// Tokenizer for bencoded stream.
struct BencodeTokenizer<R: AsyncRead + Unpin> {
    r: BufReader<R>,
}

impl<R: AsyncRead + Unpin> BencodeTokenizer<R> {
    fn new(r: R) -> Self {
        let r = BufReader::new(r);
        Self { r }
    }

    async fn next_token(&mut self) -> Result<Option<BencodeToken>> {
        loop {
            let buf = self.r.fill_buf().await?;
            match buf.first() {
                None => return Ok(None),
                Some(b'e') => {
                    self.r.consume(1);
                    return Ok(Some(BencodeToken::End));
                }
                Some(b'l') => {
                    self.r.consume(1);
                    return Ok(Some(BencodeToken::List));
                }
                Some(b'd') => {
                    self.r.consume(1);
                    return Ok(Some(BencodeToken::Dictionary));
                }
                Some(b'i') => {
                    let mut ibuf = Vec::new();
                    let n = self.r.read_until(b'e', &mut ibuf).await?;
                    if n == 0 {
                        return Err(anyhow!("unexpected end of file while reading integer"));
                    } else {
                        let num_bytes = ibuf.get(1..n - 1).context("out of bounds")?;
                        let num_str =
                            std::str::from_utf8(num_bytes).context("invalid utf8 number")?;
                        let num = i64::from_str(num_str)
                            .context("cannot parse the number {num_str:?}")?;
                        return Ok(Some(BencodeToken::Integer(num)));
                    }
                }
                Some(b'\n') => {
                    // Newlines are skipped.
                    self.r.consume(1);
                }
                Some(&x) => {
                    if x.is_ascii_digit() {
                        let mut size_buf = Vec::new();
                        let n = self.r.read_until(b':', &mut size_buf).await?;
                        if n == 0 {
                            return Err(anyhow!("unexpected end of file while reading string"));
                        } else {
                            let size_bytes = size_buf.get(0..n - 1).context("out of bounds")?;
                            let size_str =
                                std::str::from_utf8(size_bytes).context("invalid utf8 number")?;
                            let size = usize::from_str(size_str).with_context(|| {
                                format!("cannot parse length prefix {size_str:?}")
                            })?;
                            let mut str_buf = vec![0; size];
                            self.r.read_exact(&mut str_buf).await.with_context(|| {
                                format!("error while reading a string of {size} bytes")
                            })?;
                            return Ok(Some(BencodeToken::ByteString(str_buf)));
                        }
                    }
                    return Err(anyhow!("unexpected byte {x:?}"));
                }
            }
        }
    }
}

impl Sql {
    /// Deserializes the database from a bytestream.
    pub async fn deserialize(&self, r: impl AsyncRead + Unpin) -> Result<()> {
        let mut tokenizer = BencodeTokenizer::new(r);
        while let Some(token) = tokenizer.next_token().await? {
            println!("{:?}", token);
        }
        Ok(())
    }
}

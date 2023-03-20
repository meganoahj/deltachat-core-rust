//! Database deserialization module.

use std::str::FromStr;

use anyhow::{anyhow, bail, Context as _, Result};
use bstr::BString;
use rusqlite::Transaction;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::Sql;

/// Token of bencoding.
#[derive(Debug)]
enum BencodeToken {
    /// End "e".
    End,

    /// Length-prefixed bytestring.
    ByteString(BString),

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
                            return Ok(Some(BencodeToken::ByteString(BString::new(str_buf))));
                        }
                    }
                    return Err(anyhow!("unexpected byte {x:?}"));
                }
            }
        }
    }
}

struct Decoder<'a, R: AsyncRead + Unpin> {
    tx: Transaction<'a>,

    tokenizer: BencodeTokenizer<R>,
}

impl<'a, R: AsyncRead + Unpin> Decoder<'a, R> {
    fn new(tx: Transaction<'a>, r: R) -> Self {
        let tokenizer = BencodeTokenizer::new(r);
        Self { tx, tokenizer }
    }

    /// Expects a token.
    ///
    /// Returns an error on unexpected EOF.
    async fn expect_token(&mut self) -> Result<BencodeToken> {
        let token = self
            .tokenizer
            .next_token()
            .await?
            .context("unexpected end of file")?;
        Ok(token)
    }

    /// Tries to read a dictionary token.
    ///
    /// Returns an error on EOF or unexpected token.
    async fn expect_dictionary(&mut self) -> Result<()> {
        let token = self.expect_token().await?;
        match token {
            BencodeToken::Dictionary => Ok(()),
            t => Err(anyhow!("unexpected token {t:?}, expected dictionary")),
        }
    }

    /// Tries to read a list token.
    ///
    /// Returns an error on EOF or unexpected token.
    async fn expect_list(&mut self) -> Result<()> {
        let token = self.expect_token().await?;
        match token {
            BencodeToken::List => Ok(()),
            t => Err(anyhow!("unexpected token {t:?}, expected list")),
        }
    }

    async fn deserialize_config(&mut self) -> Result<()> {
        self.expect_dictionary().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn skip_until_end(&mut self) -> Result<()> {
        let mut level: usize = 0;
        loop {
            let token = self.expect_token().await?;
            match token {
                BencodeToken::End => {
                    if level == 0 {
                        return Ok(());
                    } else {
                        level -= 1;
                    }
                }
                BencodeToken::ByteString(_) | BencodeToken::Integer(_) => {}
                BencodeToken::List | BencodeToken::Dictionary => level += 1,
            }
        }
    }

    async fn skip_object(&mut self) -> Result<()> {
        let token = self.expect_token().await?;
        match token {
            BencodeToken::End => Err(anyhow!("unexpected end, expected an object")),
            BencodeToken::ByteString(_) | BencodeToken::Integer(_) => Ok(()),
            BencodeToken::List | BencodeToken::Dictionary => {
                self.skip_until_end().await?;
                Ok(())
            }
        }
    }

    async fn deserialize(mut self) -> Result<()> {
        self.expect_dictionary().await?;

        loop {
            let token = self.expect_token().await?;
            match token {
                BencodeToken::ByteString(key) => match key.as_slice() {
                    b"config" => {
                        self.deserialize_config()
                            .await
                            .context("deserialize_config")?;
                    }
                    b"contacts" => {
                        self.expect_list().await?;
                        self.skip_until_end().await?;
                    }
                    section => {
                        self.skip_object()
                            .await
                            .with_context(|| format!("skipping {section:?}"))?;
                    }
                },
                BencodeToken::End => break,
                t => {
                    bail!("unexpected token {t:?}, expected section name or end of dictionary");
                }
            }
        }

        // TODO: uncomment
        //self.tx.commit()?;
        Ok(())
    }
}

impl Sql {
    /// Deserializes the database from a bytestream.
    pub async fn deserialize(&self, r: impl AsyncRead + Unpin) -> Result<()> {
        let mut conn = self.get_connection().await?;

        // Start a write transaction to take a database snapshot.
        let transaction = conn.transaction()?;

        let decoder = Decoder::new(transaction, r);
        decoder.deserialize().await?;

        Ok(())
    }
}

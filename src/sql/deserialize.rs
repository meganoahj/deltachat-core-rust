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

struct Decoder<R: AsyncRead + Unpin> {
    tokenizer: BencodeTokenizer<R>,
}

impl<R: AsyncRead + Unpin> Decoder<R> {
    fn new(r: R) -> Self {
        let tokenizer = BencodeTokenizer::new(r);
        Self { tokenizer }
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
        match self.expect_token().await? {
            BencodeToken::Dictionary => Ok(()),
            token => Err(anyhow!("unexpected token {token:?}, expected dictionary")),
        }
    }

    /// Tries to read a list token.
    ///
    /// Returns an error on EOF or unexpected token.
    async fn expect_list(&mut self) -> Result<()> {
        match self.expect_token().await? {
            BencodeToken::List => Ok(()),
            token => Err(anyhow!("unexpected token {token:?}, expected list")),
        }
    }

    /// Tries to read a string.
    ///
    /// Returns an error on EOF or unexpected token.
    async fn expect_string(&mut self) -> Result<BString> {
        match self.expect_token().await? {
            BencodeToken::ByteString(s) => Ok(s),
            token => Err(anyhow!("unexpected token {token:?}, expected list")),
        }
    }

    /// Tries to read a string dictionary key.
    ///
    /// Returns `None` if the end of dictionary is reached.
    async fn expect_key(&mut self) -> Result<Option<BString>> {
        match self.expect_token().await? {
            BencodeToken::ByteString(key) => Ok(Some(key)),
            BencodeToken::End => Ok(None),
            token => Err(anyhow!("unexpected token {token:?}, expected string")),
        }
    }

    async fn expect_fixed_key(&mut self, expected_key: &str) -> Result<()> {
        if let Some(key) = self.expect_key().await? {
            if key.as_slice() == expected_key.as_bytes() {
                Ok(())
            } else {
                Err(anyhow!("unexpected key {key}, expected key {expected_key}"))
            }
        } else {
            Err(anyhow!(
                "unexpected end of dictionary, expected key {expected_key}"
            ))
        }
    }

    async fn expect_i64(&mut self) -> Result<i64> {
        let token = self.expect_token().await?;
        match token {
            BencodeToken::Integer(i) => Ok(i),
            t => Err(anyhow!("unexpected token {t:?}, expected integer")),
        }
    }

    async fn expect_u32(&mut self) -> Result<u32> {
        let i = u32::try_from(self.expect_i64().await?).context("failed to convert to u32")?;
        Ok(i)
    }

    async fn deserialize_config(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut dbversion_found = false;

        let mut stmt = tx.prepare("INSERT INTO config (keyname, value) VALUES (?, ?)")?;

        self.expect_dictionary().await?;
        loop {
            let token = self.expect_token().await?;
            match token {
                BencodeToken::ByteString(key) => {
                    let value = self.expect_string().await?;
                    println!("{key:?}={value:?}");

                    if key.as_slice() == b"dbversion" {
                        if dbversion_found {
                            bail!("dbversion key found twice in the config");
                        } else {
                            dbversion_found = true;
                        }

                        if value.as_slice() != b"99" {
                            bail!("unsupported serialized database version {value:?}, expected 99");
                        }
                    }

                    stmt.execute([key.as_slice(), value.as_slice()])?;
                }
                BencodeToken::End => break,
                t => return Err(anyhow!("unexpected token {t:?}, expected config key")),
            }
        }

        if !dbversion_found {
            bail!("no dbversion found in the config");
        }
        Ok(())
    }

    async fn deserialize_acpeerstates(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_chats(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_chats_contacts(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_contacts(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        loop {
            match self.expect_token().await? {
                BencodeToken::Dictionary => {
                    self.expect_fixed_key("id").await?;
                    let id = self.expect_u32().await?;

                    let mut name = None;
                    let mut authname = None;
                    let mut addr = None;
                    while let Some(key) = self.expect_key().await? {
                        match key.as_slice() {
                            b"name" => {
                                name = Some(self.expect_string().await?);
                            }
                            b"authname" => {
                                authname = Some(self.expect_string().await?);
                            }
                            b"addr" => {
                                addr = Some(self.expect_string().await?);
                            }
                        }
                    }
                }
                BencodeToken::End => break,
                token => {
                    return Err(anyhow!(
                        "unexpected token {token:?}, expected contact dictionary or end of list"
                    ))
                }
            }
        }
        Ok(())
    }

    async fn deserialize_dns_cache(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_imap(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_imap_sync(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_keypairs(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_leftgroups(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_locations(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_mdns(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_messages(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_msgs_status_updates(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_multi_device_sync(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_reactions(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }

    async fn deserialize_sending_domains(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
        self.skip_until_end().await?;
        Ok(())
    }
    async fn deserialize_tokens(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        self.expect_list().await?;
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

    async fn deserialize(mut self, mut tx: Transaction<'_>) -> Result<()> {
        self.expect_dictionary().await?;

        self.expect_fixed_key("_config").await?;
        self.deserialize_config(&mut tx)
            .await
            .context("deserialize_config")?;

        self.expect_fixed_key("acpeerstates").await?;
        self.deserialize_acpeerstates(&mut tx)
            .await
            .context("deserialize_acpeerstates")?;

        self.expect_fixed_key("chats").await?;
        self.deserialize_chats(&mut tx)
            .await
            .context("deserialize_chats")?;

        self.expect_fixed_key("chats_contacts").await?;
        self.deserialize_chats_contacts(&mut tx)
            .await
            .context("deserialize_chats_contacts")?;

        self.expect_fixed_key("contacts").await?;
        self.deserialize_contacts(&mut tx)
            .await
            .context("deserialize_contacts")?;

        self.expect_fixed_key("dns_cache").await?;
        self.deserialize_dns_cache(&mut tx)
            .await
            .context("deserialize_dns_cache")?;

        self.expect_fixed_key("imap").await?;
        self.deserialize_imap(&mut tx)
            .await
            .context("deserialize_imap")?;

        self.expect_fixed_key("imap_sync").await?;
        self.deserialize_imap_sync(&mut tx)
            .await
            .context("deserialize_imap_sync")?;

        self.expect_fixed_key("keypairs").await?;
        self.deserialize_keypairs(&mut tx)
            .await
            .context("deserialize_keypairs")?;

        self.expect_fixed_key("leftgroups").await?;
        self.deserialize_leftgroups(&mut tx)
            .await
            .context("deserialize_leftgroups")?;

        self.expect_fixed_key("locations").await?;
        self.deserialize_locations(&mut tx)
            .await
            .context("deserialize_locations")?;

        self.expect_fixed_key("mdns").await?;
        self.deserialize_mdns(&mut tx)
            .await
            .context("deserialize_mdns")?;

        self.expect_fixed_key("messages").await?;
        self.deserialize_messages(&mut tx)
            .await
            .context("deserialize_messages")?;

        self.expect_fixed_key("msgs_status_updates").await?;
        self.deserialize_msgs_status_updates(&mut tx)
            .await
            .context("deserialize_msgs_status_updates")?;

        self.expect_fixed_key("multi_device_sync").await?;
        self.deserialize_multi_device_sync(&mut tx)
            .await
            .context("deserialize_multi_device_sync")?;

        self.expect_fixed_key("reactions").await?;
        self.deserialize_reactions(&mut tx)
            .await
            .context("deserialize_reactions")?;

        self.expect_fixed_key("sending_domains").await?;
        self.deserialize_sending_domains(&mut tx)
            .await
            .context("deserialize_sending_domains")?;

        self.expect_fixed_key("tokens").await?;
        self.deserialize_tokens(&mut tx)
            .await
            .context("deserialize_tokens")?;

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

        let decoder = Decoder::new(r);
        decoder.deserialize(transaction).await?;

        Ok(())
    }
}

//! Database serialization module.
//!
//! The module contains functions to serialize database into a stream.
//!
//! Output format is based on [bencoding](http://bittorrent.org/beps/bep_0003.html)
//! with newlines added for better readability.

use anyhow::Result;
use num_traits::ToPrimitive;
use rusqlite::Transaction;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::Sql;
use crate::chat::ChatId;
use crate::constants::Chattype;
use crate::contact::{self, ContactId};

struct Encoder<'a, W: AsyncWrite + Unpin> {
    tx: Transaction<'a>,

    w: W,
}

async fn write_bytes(w: &mut (impl AsyncWrite + Unpin), b: &[u8]) -> Result<()> {
    let bytes_len = format!("{}:", b.len());
    w.write_all(bytes_len.as_bytes()).await?;
    w.write_all(b).await?;
    w.write_all(b"\n").await?;
    Ok(())
}

async fn write_str(w: &mut (impl AsyncWrite + Unpin), s: &str) -> Result<()> {
    write_bytes(w, s.as_bytes()).await?;
    Ok(())
}

async fn write_u64(w: &mut (impl AsyncWrite + Unpin), i: u64) -> Result<()> {
    let s = format!("{i}");
    w.write_all(b"i").await?;
    w.write_all(s.as_bytes()).await?;
    w.write_all(b"e\n").await?;
    Ok(())
}

async fn write_i64(w: &mut (impl AsyncWrite + Unpin), i: i64) -> Result<()> {
    let s = format!("{i}");
    w.write_all(b"i").await?;
    w.write_all(s.as_bytes()).await?;
    w.write_all(b"e\n").await?;
    Ok(())
}

async fn write_u32(w: &mut (impl AsyncWrite + Unpin), i: u32) -> Result<()> {
    let s = format!("{i}");
    w.write_all(b"i").await?;
    w.write_all(s.as_bytes()).await?;
    w.write_all(b"e\n").await?;
    Ok(())
}

async fn write_bool(w: &mut (impl AsyncWrite + Unpin), b: bool) -> Result<()> {
    if b {
        w.write_all(b"i1e\n").await?;
    } else {
        w.write_all(b"i0e\n").await?;
    }
    Ok(())
}

impl<'a, W: AsyncWrite + Unpin> Encoder<'a, W> {
    fn new(tx: Transaction<'a>, w: W) -> Self {
        Self { tx, w }
    }

    /// Serializes `config` table.
    async fn serialize_config(&mut self) -> Result<()> {
        let mut stmt = self.tx.prepare("SELECT keyname,value FROM config")?;
        let mut rows = stmt.query(())?;
        self.w.write_all(b"d\n").await?;
        while let Some(row) = rows.next()? {
            let keyname: String = row.get(0)?;
            let value: String = row.get(1)?;
            write_str(&mut self.w, &keyname).await?;
            write_str(&mut self.w, &value).await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    /// Serializes contacts.
    async fn serialize_contacts(&mut self) -> Result<()> {
        let mut stmt = self.tx.prepare(
            "SELECT \
        id,\
        name,\
        addr,\
        origin,\
        blocked,\
        last_seen,\
        param,\
        authname,\
        selfavatar_sent,\
        status FROM contacts",
        )?;
        let mut rows = stmt.query(())?;
        self.w.write_all(b"l").await?;
        while let Some(row) = rows.next()? {
            let id: ContactId = row.get("id")?;
            let name: String = row.get("name")?;
            let authname: String = row.get("authname")?;
            let addr: String = row.get("addr")?;
            let origin: contact::Origin = row.get("origin")?;
            let origin = origin.to_u32();
            let blocked: Option<bool> = row.get("blocked")?;
            let blocked = blocked.unwrap_or_default();
            let last_seen: i64 = row.get("last_seen")?;
            let selfavatar_sent: i64 = row.get("selfavatar_sent")?;
            let param: String = row.get("param")?;
            let status: Option<String> = row.get("status")?;

            self.w.write_all(b"d\n").await?;

            write_str(&mut self.w, "id").await?;
            write_u32(&mut self.w, id.to_u32()).await?;

            write_str(&mut self.w, "name").await?;
            write_str(&mut self.w, &name).await?;

            write_str(&mut self.w, "addr").await?;
            write_str(&mut self.w, &addr).await?;

            if let Some(origin) = origin {
                write_str(&mut self.w, "origin").await?;
                write_u32(&mut self.w, origin).await?;
            }

            write_str(&mut self.w, "blocked").await?;
            write_bool(&mut self.w, blocked).await?;

            write_str(&mut self.w, "last_seen").await?;
            write_i64(&mut self.w, last_seen).await?;

            // TODO: parse param instead of serializeing as is
            write_str(&mut self.w, "param").await?;
            write_str(&mut self.w, &param).await?;

            write_str(&mut self.w, "authname").await?;
            write_str(&mut self.w, &authname).await?;

            write_str(&mut self.w, "selfavatar_sent").await?;
            write_i64(&mut self.w, selfavatar_sent).await?;

            if let Some(status) = status {
                if !status.is_empty() {
                    write_str(&mut self.w, "status").await?;
                    write_str(&mut self.w, &status).await?;
                }
            }
            self.w.write_all(b"e\n").await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    /// Serializes chats.
    async fn serialize_chats(&mut self) -> Result<()> {
        let mut stmt = self.tx.prepare(
            "SELECT \
        id,\
        type,\
        name,\
        blocked,\
        grpid,\
        param,\
        archived,\
        gossiped_timestamp,\
        locations_send_begin,\
        locations_send_until,\
        locations_last_sent,\
        created_timestamp,\
        muted_until,\
        ephemeral_timer,\
        protected FROM chats",
        )?;
        let mut rows = stmt.query(())?;

        self.w.write_all(b"l\n").await?;
        while let Some(row) = rows.next()? {
            let id: ChatId = row.get("id")?;
            let typ: Chattype = row.get("type")?;

            self.w.write_all(b"d\n").await?;
            write_str(&mut self.w, "id").await?;
            write_u32(&mut self.w, id.to_u32()).await?;

            if let Some(typ) = typ.to_u32() {
                write_str(&mut self.w, "type").await?;
                write_u32(&mut self.w, typ).await?;
            }

            self.w.write_all(b"e\n").await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    async fn serialize_leftgroups(&mut self) -> Result<()> {
        // TODO leftgrps
        self.w.write_all(b"le\n").await?;
        Ok(())
    }

    async fn serialize_keypairs(&mut self) -> Result<()> {
        Ok(())
    }

    async fn serialize(&mut self) -> Result<()> {
        self.w.write_all(b"d\n").await?;
        write_str(&mut self.w, "config").await?;

        self.serialize_config().await?;
        write_str(&mut self.w, "contacts").await?;
        self.serialize_contacts().await?;
        write_str(&mut self.w, "chats").await?;
        self.serialize_chats().await?;
        write_str(&mut self.w, "leftgroups").await?;
        self.serialize_leftgroups().await?;
        write_str(&mut self.w, "keypairs").await?;
        self.serialize_keypairs().await?;

        // TODO msgs_mdns
        // TODO tokens
        // TODO locations
        // TODO devmsglabels
        // TODO msgs
        // TODO imap_sync
        // TODO multi_device_sync
        // TODO imap
        // TODO msgs_status_updates
        // TODO bobstate
        // TODO imap_markseen
        // TODO smtp_mdns
        // TODO smtp_status_updates
        // TODO reactions
        // TODO sending_domains
        // TODO acpeerstates
        // TODO chats_contacts
        // TODO dns_cache

        // jobs table is skipped
        // smtp table is skipped, it is SMTP queue.
        self.w.write_all(b"e\n").await?;
        Ok(())
    }
}

impl Sql {
    pub async fn serialize(&self, w: impl AsyncWrite + Unpin) -> Result<()> {
        let mut conn = self.get_connection().await?;

        // Start a read transaction to take a database snapshot.
        let transaction = conn.transaction()?;
        let mut encoder = Encoder::new(transaction, w);
        encoder.serialize().await?;
        Ok(())
    }
}

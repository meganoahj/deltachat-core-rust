//! Database serialization module.
//!
//! The module contains functions to serialize database into a stream.
//!
//! Output format is based on [bencoding](http://bittorrent.org/beps/bep_0003.html)
//! with newlines added for better readability.

use anyhow::{Result, Context as _};
use num_traits::ToPrimitive;
use rusqlite::Transaction;
use rusqlite::types::ValueRef;
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
        let mut stmt = self.tx.prepare("SELECT grpid FROM leftgrps")?;
        let mut rows = stmt.query(())?;

        self.w.write_all(b"l").await?;
        while let Some(row) = rows.next()? {
            let grpid: String = row.get("grpid")?;
            write_str(&mut self.w, &grpid).await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    async fn serialize_keypairs(&mut self) -> Result<()> {
        let mut stmt = self
            .tx
            .prepare("SELECT id,addr,is_default,private_key,public_key,created FROM keypairs")?;
        let mut rows = stmt.query(())?;

        self.w.write_all(b"l\n").await?;
        while let Some(row) = rows.next()? {
            let id: u32 = row.get("id")?;
            let addr: String = row.get("addr")?;
            let is_default: u32 = row.get("is_default")?;
            let is_default = is_default != 0;
            let private_key: Vec<u8> = row.get("private_key")?;
            let public_key: Vec<u8> = row.get("public_key")?;
            let created: i64 = row.get("created")?;

            self.w.write_all(b"d\n").await?;

            write_str(&mut self.w, "id").await?;
            write_u32(&mut self.w, id).await?;

            write_str(&mut self.w, "type").await?;
            write_str(&mut self.w, &addr).await?;

            write_str(&mut self.w, "is_default").await?;
            write_bool(&mut self.w, is_default).await?;

            write_str(&mut self.w, "private_key").await?;
            write_bytes(&mut self.w, &private_key).await?;

            write_str(&mut self.w, "public_key").await?;
            write_bytes(&mut self.w, &public_key).await?;

            write_str(&mut self.w, "created").await?;
            write_i64(&mut self.w, created).await?;

            self.w.write_all(b"e\n").await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    /// Serializes messages.
    async fn serialize_messages(&mut self) -> Result<()> {
        let mut stmt = self.tx.prepare(
            "SELECT
                        id,
                        rfc724_mid,
                        chat_id,
                        from_id, to_id,
                        timestamp,
                        type,
                        state,
                        msgrmsg,
                        bytes,
                        txt,
                        txt_raw,
                        param,
                        timestamp_sent,
                        timestamp_rcvd,
                        hidden,
                        mime_headers,
                        mime_in_reply_to,
                        mime_references,
                        location_id FROM msgs",
        )?;
        let mut rows = stmt.query(())?;

        self.w.write_all(b"l\n").await?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get("id")?;
            let rfc724_mid: String = row.get("rfc724_mid")?;
            let chat_id: i64 = row.get("chat_id")?;
            let from_id: i64 = row.get("from_id")?;
            let to_id: i64 = row.get("to_id")?;
            let timestamp: i64 = row.get("timestamp")?;
            let typ: i64 = row.get("type")?;
            let state: i64 = row.get("state")?;
            let msgrmsg: i64 = row.get("msgrmsg")?;
            let bytes: i64 = row.get("bytes")?;
            let txt: String = row.get("txt")?;
            let txt_raw: String = row.get("txt_raw")?;
            let param: String = row.get("param")?;
            let timestamp_sent: i64 = row.get("timestamp_sent")?;
            let timestamp_rcvd: i64 = row.get("timestamp_rcvd")?;
            let hidden: i64 = row.get("hidden")?;
            let mime_headers: Vec<u8> =
                row.get("mime_headers")
                    .or_else(|err| match row.get_ref("mime_headers")? {
                        ValueRef::Null => Ok(Vec::new()),
                        ValueRef::Text(text) => Ok(text.to_vec()),
                        ValueRef::Blob(blob) => Ok(blob.to_vec()),
                        ValueRef::Integer(_) | ValueRef::Real(_) => Err(err),
                    })?;
            let mime_in_reply_to: Option<String> = row.get("mime_in_reply_to")?;
            let mime_references: Option<String> = row.get("mime_references")?;
            let location_id: i64 = row.get("location_id")?;

            self.w.write_all(b"d\n").await?;

            write_str(&mut self.w, "id").await?;
            write_i64(&mut self.w, id).await?;

            write_str(&mut self.w, "rfc724_mid").await?;
            write_str(&mut self.w, &rfc724_mid).await?;

            write_str(&mut self.w, "chat_id").await?;
            write_i64(&mut self.w, chat_id).await?;

            write_str(&mut self.w, "from_id").await?;
            write_i64(&mut self.w, from_id).await?;

            write_str(&mut self.w, "to_id").await?;
            write_i64(&mut self.w, to_id).await?;

            write_str(&mut self.w, "timestamp").await?;
            write_i64(&mut self.w, timestamp).await?;

            write_str(&mut self.w, "type").await?;
            write_i64(&mut self.w, typ).await?;

            write_str(&mut self.w, "state").await?;
            write_i64(&mut self.w, state).await?;

            write_str(&mut self.w, "msgrmsg").await?;
            write_i64(&mut self.w, msgrmsg).await?;

            write_str(&mut self.w, "bytes").await?;
            write_i64(&mut self.w, bytes).await?;

            write_str(&mut self.w, "txt").await?;
            write_str(&mut self.w, &txt).await?;

            write_str(&mut self.w, "txt_raw").await?;
            write_str(&mut self.w, &txt_raw).await?;

            // TODO split into parts instead of writing as is
            write_str(&mut self.w, "param").await?;
            write_str(&mut self.w, &param).await?;

            write_str(&mut self.w, "timestamp_sent").await?;
            write_i64(&mut self.w, timestamp_sent).await?;

            write_str(&mut self.w, "timestamp_rcvd").await?;
            write_i64(&mut self.w, timestamp_rcvd).await?;

            write_str(&mut self.w, "hidden").await?;
            write_i64(&mut self.w, hidden).await?;

            write_str(&mut self.w, "mime_headers").await?;
            write_bytes(&mut self.w, &mime_headers).await?;

            if let Some(mime_in_reply_to) = mime_in_reply_to {
                write_str(&mut self.w, "mime_in_reply_to").await?;
                write_str(&mut self.w, &mime_in_reply_to).await?;
            }

            if let Some(mime_references) = mime_references {
                write_str(&mut self.w, "mime_references").await?;
                write_str(&mut self.w, &mime_references).await?;
            }

            write_str(&mut self.w, "location_id").await?;
            write_i64(&mut self.w, location_id).await?;

            self.w.write_all(b"e\n").await?;
        }
        self.w.write_all(b"e\n").await?;
        Ok(())
    }

    /// Serializes MDNs.
    async fn serialize_mdns(&mut self) -> Result<()> {
        let mut stmt = self
            .tx
            .prepare("SELECT msg_id, contact_id, timestamp_sent FROM msgs_mdns")?;
        let mut rows = stmt.query(())?;

        self.w.write_all(b"l\n").await?;
        while let Some(row) = rows.next()? {
            let msg_id: u32 = row.get("msg_id")?;
            let contact_id: u32 = row.get("contact_id")?;
            let timestamp_sent: i64 = row.get("timestamp_sent")?;

            self.w.write_all(b"d\n").await?;

            write_str(&mut self.w, "msg_id").await?;
            write_u32(&mut self.w, msg_id).await?;

            write_str(&mut self.w, "contact_id").await?;
            write_u32(&mut self.w, contact_id).await?;

            write_str(&mut self.w, "timestamp_sent").await?;
            write_i64(&mut self.w, timestamp_sent).await?;

            self.w.write_all(b"e\n").await?;
        }
        self.w.write_all(b"e\n").await?;
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

        write_str(&mut self.w, "messages").await?;
        self.serialize_messages().await.context("serialize messages")?;

        write_str(&mut self.w, "mdns").await?;
        self.serialize_mdns().await?;

        // TODO tokens
        // TODO locations
        // TODO devmsglabels
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
    /// Serializes the database into a bytestream.
    pub async fn serialize(&self, w: impl AsyncWrite + Unpin) -> Result<()> {
        let mut conn = self.get_connection().await?;

        // Start a read transaction to take a database snapshot.
        let transaction = conn.transaction()?;
        let mut encoder = Encoder::new(transaction, w);
        encoder.serialize().await?;
        Ok(())
    }
}

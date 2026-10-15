//! Types the paste keys into whatever app has focus.
//!
//! Wayland has no way to inject keys directly, so this goes through the
//! RemoteDesktop portal. The first use shows the system's one-time consent
//! dialog; after that the portal remembers the choice (restore token).

use std::{collections::HashMap, env, fs, path::PathBuf, thread, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_lite::StreamExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use zbus::{
    Connection, Proxy,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use crate::store::PasteKeys;

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";

const KEY_LEFTCTRL: i32 = 29;
const KEY_LEFTSHIFT: i32 = 42;
const KEY_V: i32 = 47;
const KEY_INSERT: i32 = 110;

#[derive(Clone)]
pub struct Paster {
    tx: UnboundedSender<PasteKeys>,
}

impl Paster {
    pub fn spawn() -> Self {
        let (tx, mut rx) = unbounded_channel::<PasteKeys>();
        thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("lclip: paste disabled: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let mut session: Option<Session> = None;
                while let Some(keys) = rx.recv().await {
                    if session.is_none() {
                        match Session::open().await {
                            Ok(s) => session = Some(s),
                            Err(e) => {
                                eprintln!("lclip: cannot auto paste ({e:#}); the entry is on the clipboard, press paste yourself");
                                continue;
                            }
                        }
                    }
                    if let Err(e) = session.as_ref().unwrap().press(keys).await {
                        eprintln!("lclip: paste failed: {e:#}");
                        session = None;
                    }
                }
            });
        });
        Self { tx }
    }

    pub fn press(&self, keys: PasteKeys) {
        let _ = self.tx.send(keys);
    }
}

struct Session {
    rd: Proxy<'static>,
    handle: String,
}

fn token_file() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(env::temp_dir)
        .join("lclip/portal-token")
}

impl Session {
    async fn open() -> Result<Self> {
        let conn = Connection::session().await.context("no session bus")?;
        let rd = Proxy::new(&conn, DEST, PATH, RD).await.context("no desktop portal")?;

        let t = |n: &str| format!("lclip_{n}_{}", std::process::id());

        // 1. session
        let opts: HashMap<&str, Value<'_>> = HashMap::from([
            ("handle_token", Value::from(t("create"))),
            ("session_handle_token", Value::from(t("session"))),
        ]);
        let res = call(&conn, &rd, &t("create"), "CreateSession", &(opts,)).await?;
        let handle: String = res
            .get("session_handle")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok())
            .context("portal gave no session")?;
        let session = ObjectPath::try_from(handle.as_str())?;

        // 2. ask for a keyboard, and to be remembered
        let mut opts: HashMap<&str, Value<'_>> = HashMap::from([
            ("handle_token", Value::from(t("select"))),
            ("types", Value::from(1u32)),
            ("persist_mode", Value::from(2u32)),
        ]);
        if let Ok(token) = fs::read_to_string(token_file()) {
            opts.insert("restore_token", Value::from(token.trim().to_string()));
        }
        call(&conn, &rd, &t("select"), "SelectDevices", &(&session, opts)).await?;

        // 3. start (the consent dialog on first use)
        let opts: HashMap<&str, Value<'_>> = HashMap::from([("handle_token", Value::from(t("start")))]);
        let res = call(&conn, &rd, &t("start"), "Start", &(&session, "", opts)).await?;
        if let Some(token) = res
            .get("restore_token")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok())
        {
            let path = token_file();
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(path, token);
        }

        Ok(Self { rd, handle })
    }

    async fn key(&self, code: i32, down: bool) -> Result<()> {
        let session = ObjectPath::try_from(self.handle.as_str())?;
        let opts: HashMap<&str, Value<'_>> = HashMap::new();
        self.rd
            .call_method("NotifyKeyboardKeycode", &(&session, opts, code, down as u32))
            .await?;
        Ok(())
    }

    async fn press(&self, keys: PasteKeys) -> Result<()> {
        let seq: &[i32] = match keys {
            PasteKeys::CtrlV => &[KEY_LEFTCTRL, KEY_V],
            PasteKeys::CtrlShiftV => &[KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_V],
            PasteKeys::ShiftInsert => &[KEY_LEFTSHIFT, KEY_INSERT],
        };
        for &k in seq {
            self.key(k, true).await?;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
        for &k in seq.iter().rev() {
            self.key(k, false).await?;
        }
        Ok(())
    }
}

/// One portal call: subscribe to its Response first, call, then wait for the answer.
async fn call<B>(
    conn: &Connection,
    rd: &Proxy<'_>,
    token: &str,
    method: &str,
    body: &B,
) -> Result<HashMap<String, OwnedValue>>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let sender = conn
        .unique_name()
        .ok_or_else(|| anyhow!("no bus name"))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let req = Proxy::new(conn, DEST, path, "org.freedesktop.portal.Request").await?;
    let mut responses = req.receive_signal("Response").await?;

    rd.call_method(method, body).await?;

    let msg = tokio::time::timeout(Duration::from_secs(120), responses.next())
        .await
        .map_err(|_| anyhow!("{method}: no answer from the portal"))?
        .ok_or_else(|| anyhow!("{method}: portal closed"))?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if code != 0 {
        bail!("{method}: the request was declined");
    }
    Ok(results)
}

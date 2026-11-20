//! Winlink bridge hooks for the engine. The gateway itself (joining mail and attachments,
//! the on-disk spool) is `kahf_winlink::gateway`; the CMS sync is the separate
//! `kahf-winlink gateway` process on the same folder. Off until a gateway is set.

use super::{lock, Engine, Worker};
use kahf_core::session::Message;
use kahf_winlink::gateway::{Taken, WinlinkGateway};
use std::fs;
use std::sync::Arc;

impl Engine {
    /// Turns the Winlink bridge on (or off with `None`). Mail from the gateway's approved
    /// callsigns to an email or `@winlink.org` address is then queued for Winlink instead of
    /// being filed as inbox mail, and `pump_winlink` delivers Winlink mail to them.
    pub fn set_winlink_gateway(&self, gateway: Option<Arc<WinlinkGateway>>) {
        *lock(&self.shared.winlink) = gateway;
    }

    /// Queues Winlink mail waiting for the gateway's approved callsigns as kahf mail to them.
    /// Call it now and then (once a minute is plenty). Returns how many were queued.
    pub fn pump_winlink(&mut self) -> usize {
        let Some(gw) = lock(&self.shared.winlink).clone() else {
            return 0;
        };
        let dir = self.config.data_dir.join("winlink-out");
        gw.pump_inbound(|m| {
            // The kahf mail comes from this station, so name the real sender in the body.
            let body = format!("Winlink mail from {}\n\n{}", m.from, m.body);
            let sub = fs::create_dir_all(&dir)
                .and_then(|_| {
                    let d = dir.join(format!("{}-{}", m.to, m.subject.len()));
                    fs::create_dir_all(&d).map(|_| d)
                })
                .map_err(|e| e.to_string())?;
            let mut paths = Vec::new();
            for (name, data) in &m.files {
                let safe: String = name.chars().filter(|c| !matches!(c, '/' | '\\' | '\0')).collect();
                let p = sub.join(if safe.is_empty() {
                    "attachment".to_string()
                } else {
                    safe
                });
                fs::write(&p, data).map_err(|e| e.to_string())?;
                paths.push(p);
            }
            let r = self.send_mail(&m.to, &m.subject, &body, &paths).map(|_| ());
            let _ = fs::remove_dir_all(&sub);
            r
        })
    }
}

impl Worker {
    /// Offers a received message to the Winlink gateway. True when the gateway took it.
    pub(super) fn offer_to_winlink(&self, msg: &Message) -> bool {
        let Some(gw) = lock(&self.shared.winlink).clone() else {
            return false;
        };
        match gw.take(msg, super::now_unix_secs()) {
            Taken::No => false,
            Taken::Held => true,
            Taken::Queued(mid) => {
                self.log(format!("mail queued for Winlink ({mid})"));
                true
            }
            Taken::Refused(why) => {
                self.log(format!("mail for Winlink refused: {why}"));
                true
            }
        }
    }
}

// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
//! Exclusive service ownership and fail-closed monitoring on the same bus.
use crate::api;
use futures_util::StreamExt;
use zbus::{Connection, MatchRule, MessageStream, fdo::RequestNameFlags, message::Type};

// Subscribe before requesting the name so a loss during startup is not missed.
pub async fn loss_stream(connection: &Connection) -> zbus::Result<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")?
        .path("/org/freedesktop/DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameLost")?
        .add_arg(api::NAME)?
        .build();
    MessageStream::for_match_rule(rule, connection, Some(8)).await
}

pub async fn request(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    connection
        .request_name_with_flags(api::NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .map_err(|e| {
            format!(
                "Cannot own {} (another instance may already be running): {e}",
                api::NAME
            )
        })?;
    Ok(())
}

pub async fn wait_for_loss(mut stream: MessageStream) -> String {
    match stream.next().await {
        Some(Ok(_)) => format!(
            "Lost D-Bus name {}; exiting and flushing history",
            api::NAME
        ),
        Some(Err(e)) => {
            format!("Service ownership monitor failed: {e}; exiting and flushing history")
        }
        None => "Service bus disconnected; exiting and flushing history".into(),
    }
}

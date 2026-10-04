//! Connection setup shared by every connection the app opens.

use std::{path::Path, time::Duration};

use rusqlite::Connection;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    crate::vector::register();
    let conn = Connection::open(path)?;
    configure(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    // WAL: readers don't block the writer; NORMAL is durable enough with WAL and much faster.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Deleted content (removed documents, chats) is overwritten instead of lingering in free pages.
    conn.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}

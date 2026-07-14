use crate::{
    begin_immediate, finish_tx, map_turso_error,
    row::{int, text},
};
use quarry_core::Result;
use turso::Connection;

pub(crate) async fn ensure_links_resolution_status_column(conn: &Connection) -> Result<()> {
    let mut rows = conn
        .query("PRAGMA table_info(links)", ())
        .await
        .map_err(map_turso_error)?;
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        if text(&row, 1)? == "resolution_status" {
            return Ok(());
        }
    }
    conn.execute(
        "ALTER TABLE links ADD COLUMN resolution_status TEXT NOT NULL DEFAULT 'unresolved'",
        (),
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

pub(crate) async fn ensure_documents_created_ip_address_column(conn: &Connection) -> Result<()> {
    let columns = table_columns_conn(conn, "documents").await?;
    if columns
        .iter()
        .any(|column| column.name == "created_ip_address")
    {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE documents ADD COLUMN created_ip_address TEXT",
        (),
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

pub(crate) async fn migrate_documents_scope_ttl(conn: &Connection) -> Result<()> {
    let columns = table_columns_conn(conn, "documents").await?;
    let has_scope = columns.iter().any(|column| column.name == "document_scope");
    let has_expires_at = columns.iter().any(|column| column.name == "expires_at");
    let has_created_ip_address = columns
        .iter()
        .any(|column| column.name == "created_ip_address");
    let library_id_not_null = columns
        .iter()
        .find(|column| column.name == "library_id")
        .is_some_and(|column| column.not_null);
    if has_scope
        && has_expires_at
        && !library_id_not_null
        && !documents_has_legacy_path_unique_conn(conn).await?
    {
        return Ok(());
    }

    begin_immediate(conn).await?;
    let result = async {
        let scope_expr = if has_scope {
            "document_scope"
        } else {
            "'library'"
        };
        let expires_expr = if has_expires_at { "expires_at" } else { "NULL" };
        let created_ip_address_expr = if has_created_ip_address {
            "created_ip_address"
        } else {
            "NULL"
        };
        let insert_sql = format!(
            r#"
            INSERT INTO documents
              (id, library_id, path, head_version_id, deleted_at, created_at, updated_at, document_scope, expires_at, created_ip_address)
            SELECT id, library_id, path, head_version_id, deleted_at, created_at, updated_at,
                   {scope_expr}, {expires_expr}, {created_ip_address_expr}
            FROM documents_scope_ttl_migration;
            "#
        );
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS documents_scope_ttl_migration;
            ALTER TABLE documents RENAME TO documents_scope_ttl_migration;
            CREATE TABLE documents(
              id TEXT PRIMARY KEY,
              library_id TEXT,
              path TEXT NOT NULL,
              head_version_id TEXT,
              deleted_at TEXT,
              document_scope TEXT NOT NULL DEFAULT 'library',
              expires_at TEXT,
              created_ip_address TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              CHECK (document_scope IN ('library', 'tmp')),
              CHECK (
                (document_scope = 'library' AND library_id IS NOT NULL)
                OR (document_scope = 'tmp' AND library_id IS NULL AND expires_at IS NOT NULL)
              )
            );
            "#,
        )
        .await
        .map_err(map_turso_error)?;
        conn.execute_batch(&insert_sql).await.map_err(map_turso_error)?;
        conn.execute("DROP TABLE documents_scope_ttl_migration", ())
            .await
            .map_err(map_turso_error)?;
        Ok(())
    }
    .await;
    finish_tx(conn, result).await
}

pub(crate) async fn ensure_document_indexes_conn(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DROP INDEX IF EXISTS idx_documents_active_library_path;
        DROP INDEX IF EXISTS idx_documents_active_tmp_path;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_active_library_path
          ON documents(library_id, path)
          WHERE document_scope = 'library' AND deleted_at IS NULL AND head_version_id IS NOT NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_active_tmp_path
          ON documents(path)
          WHERE document_scope = 'tmp' AND library_id IS NULL AND deleted_at IS NULL AND head_version_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_documents_library_path ON documents(library_id, path);
        CREATE INDEX IF NOT EXISTS idx_documents_scope_path ON documents(document_scope, path);
        CREATE INDEX IF NOT EXISTS idx_documents_expires_at ON documents(expires_at);
        CREATE INDEX IF NOT EXISTS idx_documents_created_at ON documents(created_at);
        CREATE INDEX IF NOT EXISTS idx_documents_created_ip_address_created_at
          ON documents(created_ip_address, created_at)
          WHERE created_ip_address IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_documents_updated_at ON documents(updated_at);
        "#,
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

struct TableColumn {
    name: String,
    not_null: bool,
}

async fn table_columns_conn(conn: &Connection, table: &str) -> Result<Vec<TableColumn>> {
    let mut rows = conn
        .query(
            format!("PRAGMA table_info({})", quote_sql_string(table)),
            (),
        )
        .await
        .map_err(map_turso_error)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        columns.push(TableColumn {
            name: text(&row, 1)?,
            not_null: int(&row, 3)? != 0,
        });
    }
    Ok(columns)
}

async fn documents_has_legacy_path_unique_conn(conn: &Connection) -> Result<bool> {
    let mut rows = conn
        .query("PRAGMA index_list('documents')", ())
        .await
        .map_err(map_turso_error)?;
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        let name = text(&row, 1)?;
        if name == "idx_documents_active_library_path" || int(&row, 2)? == 0 {
            continue;
        }
        if index_columns_conn(conn, &name).await? == ["library_id", "path"] {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn index_columns_conn(conn: &Connection, index_name: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            format!("PRAGMA index_info({})", quote_sql_string(index_name)),
            (),
        )
        .await
        .map_err(map_turso_error)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        columns.push(text(&row, 2)?);
    }
    Ok(columns)
}

fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

use crate::{QuarryStore, ensure_inode_conn, map_turso_error, row::library_from_row};
use quarry_core::{Library, QuarryError, Result, now_timestamp};
use turso::{Connection, params};
use uuid::Uuid;

impl QuarryStore {
    pub async fn create_library(&self, slug: &str) -> Result<Library> {
        validate_slug(slug)?;
        let slug = slug.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                if let Some(existing) = Self::library_by_slug_or_id_conn(conn, &slug).await? {
                    return Ok(existing);
                }
                let now = now_timestamp();
                let library = Library {
                    id: Uuid::new_v4().to_string(),
                    slug,
                    created_at: now.into(),
                    settings: serde_json::json!({}),
                };
                conn.execute(
                    "INSERT INTO libraries (id, slug, created_at, settings_json) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        library.id.clone(),
                        library.slug.clone(),
                        library.created_at.to_string(),
                        library.settings.to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                ensure_inode_conn(conn, &library.id, "").await?;
                Ok(library)
            })
        })
        .await
    }

    pub async fn list_libraries(&self) -> Result<Vec<Library>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, slug, created_at, settings_json FROM libraries ORDER BY slug",
                (),
            )
            .await
            .map_err(map_turso_error)?;
        let mut libraries = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            libraries.push(library_from_row(&row)?);
        }
        Ok(libraries)
    }

    pub async fn get_library(&self, slug_or_id: &str) -> Result<Library> {
        let conn = self.conn()?;
        Self::library_by_slug_or_id_conn(&conn, slug_or_id)
            .await?
            .ok_or_else(|| QuarryError::NotFound(format!("library {slug_or_id}")))
    }

    pub(crate) async fn library_by_slug_or_id_conn(
        conn: &Connection,
        slug_or_id: &str,
    ) -> Result<Option<Library>> {
        let mut rows = conn
            .query(
                "SELECT id, slug, created_at, settings_json FROM libraries WHERE slug = ?1 OR id = ?1 LIMIT 1",
                params![slug_or_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some(library_from_row(&row)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn require_library_conn(
        conn: &Connection,
        slug_or_id: &str,
    ) -> Result<Library> {
        Self::library_by_slug_or_id_conn(conn, slug_or_id)
            .await?
            .ok_or_else(|| QuarryError::NotFound(format!("library {slug_or_id}")))
    }
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.contains('/')
        || slug.contains('\\')
        || slug == "."
        || slug == ".."
        || slug.chars().any(char::is_whitespace)
    {
        Err(QuarryError::InvalidPath(format!("library slug {slug}")))
    } else {
        Ok(())
    }
}

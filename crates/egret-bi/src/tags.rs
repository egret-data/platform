//! Classification tag repository (Access P3.1) — the metadata foundation for
//! ABAC. A `tags` catalog + `tag_assignments` (which objects carry which tags).
//!
//! METADATA ONLY: no authorization decision reads these tables in this phase.
//! Enforcement (tags feeding access decisions) is a later phase — this repo just
//! stores and lists the classification metadata.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct TagRepo {
    db: Db,
}

/// A classification tag plus its live assignment count (0 unless `list`/`get`
/// computed it via the LEFT JOIN).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Tag {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub color: String,
    /// Number of `tag_assignments` rows referencing this tag.
    pub assignment_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The tag columns joined with a computed assignment count. `created_by` is
/// intentionally omitted from the domain type (audit-only column).
const SELECT_WITH_COUNT: &str = "SELECT t.id, t.key, t.name, t.description, t.color, \
     COUNT(ta.tag_id) AS assignment_count, t.created_at \
     FROM tags t LEFT JOIN tag_assignments ta ON ta.tag_id = t.id";

fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a tag with that key already exists".into());
        }
    }
    BiError::Db(e)
}

/// A tag key must be a stable, URL/identifier-safe slug: non-empty, lowercase
/// ascii alphanumerics plus `:`, `-`. Keeps keys usable as ABAC selector tokens
/// later without escaping.
fn validate_key(key: &str) -> Result<(), BiError> {
    if key.is_empty() {
        return Err(BiError::BadRequest("tag key must not be empty".into()));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ':' || c == '-')
    {
        return Err(BiError::BadRequest(format!(
            "tag key must be lowercase ascii alphanumerics, ':' or '-': {key}"
        )));
    }
    Ok(())
}

impl TagRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create a tag. Validates `key`; maps the unique-key violation to `Conflict`.
    pub async fn create(
        &self,
        key: &str,
        name: &str,
        description: &str,
        color: &str,
        created_by: Option<Uuid>,
    ) -> Result<Tag, BiError> {
        validate_key(key)?;
        // RETURNING can't carry the join-computed count, so INSERT then re-select
        // (a freshly created tag always has assignment_count = 0).
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO tags (key, name, description, color, created_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(key)
        .bind(name)
        .bind(description)
        .bind(color)
        .bind(created_by)
        .fetch_one(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        self.get(id)
            .await?
            .ok_or_else(|| BiError::NotFound(format!("tag {id}")))
    }

    /// All tags, each with its assignment count, ordered by key.
    pub async fn list(&self) -> Result<Vec<Tag>, BiError> {
        let sql = format!("{SELECT_WITH_COUNT} GROUP BY t.id ORDER BY t.key");
        Ok(sqlx::query_as::<_, Tag>(&sql)
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// One tag by id (with its assignment count), or `None`.
    pub async fn get(&self, id: Uuid) -> Result<Option<Tag>, BiError> {
        let sql = format!("{SELECT_WITH_COUNT} WHERE t.id = $1 GROUP BY t.id");
        Ok(sqlx::query_as::<_, Tag>(&sql)
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    /// Update the mutable fields; `None` leaves a field unchanged (COALESCE). The
    /// key is immutable (it's the stable selector token). Returns the updated tag
    /// or `None` if no such id.
    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<Option<Tag>, BiError> {
        let affected = sqlx::query(
            "UPDATE tags SET \
                name = COALESCE($2, name), \
                description = COALESCE($3, description), \
                color = COALESCE($4, color) \
             WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(color)
        .execute(&self.db.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get(id).await
    }

    /// Delete a tag; its `tag_assignments` cascade (FK ON DELETE CASCADE).
    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM tags WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Assign a tag to an object. Idempotent — a re-assign is a no-op (ON CONFLICT
    /// DO NOTHING on the composite primary key).
    pub async fn assign(
        &self,
        tag_id: Uuid,
        object_type: &str,
        object_id: Uuid,
        assigned_by: Option<Uuid>,
    ) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO tag_assignments (tag_id, object_type, object_id, assigned_by) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (tag_id, object_type, object_id) DO NOTHING",
        )
        .bind(tag_id)
        .bind(object_type)
        .bind(object_id)
        .bind(assigned_by)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Remove a tag from an object (idempotent — deletes zero or one row).
    pub async fn unassign(
        &self,
        tag_id: Uuid,
        object_type: &str,
        object_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query(
            "DELETE FROM tag_assignments \
             WHERE tag_id = $1 AND object_type = $2 AND object_id = $3",
        )
        .bind(tag_id)
        .bind(object_type)
        .bind(object_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// The tags carried by one object, ordered by key. `assignment_count` here is
    /// the tag's global count (LEFT JOIN over all assignments), not per-object.
    pub async fn tags_for_object(
        &self,
        object_type: &str,
        object_id: Uuid,
    ) -> Result<Vec<Tag>, BiError> {
        let sql = format!(
            "{SELECT_WITH_COUNT} \
             JOIN tag_assignments this ON this.tag_id = t.id \
                 AND this.object_type = $1 AND this.object_id = $2 \
             GROUP BY t.id ORDER BY t.key"
        );
        Ok(sqlx::query_as::<_, Tag>(&sql)
            .bind(object_type)
            .bind(object_id)
            .fetch_all(&self.db.pool)
            .await?)
    }
}

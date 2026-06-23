CREATE TABLE bookmarks (
  user_id     uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  entity_type text        NOT NULL,
  entity_id   uuid        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, entity_type, entity_id)
);
CREATE INDEX bookmarks_user_idx ON bookmarks (user_id, entity_type);

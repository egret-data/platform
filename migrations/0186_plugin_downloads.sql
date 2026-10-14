-- View governance B4: what each Ranger plugin last downloaded from egret.
--
-- egret IS the Ranger Admin, so every plugin poll lands here. One row per
-- (service the poll names, plugin id, kind). `last_known_version` is the
-- version the plugin REPORTED holding on its latest poll; that, and only that,
-- is evidence the plugin applied a version. `served_version` is what egret
-- answered with on a 200, kept for diagnosis; a served version is never
-- counted as held, because the plugin may fail to apply it.
--
-- The tag-service row is not what Trino polls: Trino polls tags under its
-- RESOURCE service, so `service_id` is the service the poll names.

CREATE TABLE policy.ranger_plugin_downloads (
    service_id         bigint      NOT NULL REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    plugin_id          text        NOT NULL,
    kind               text        NOT NULL CHECK (kind IN ('policies', 'tags')),
    last_known_version bigint,
    served_version     bigint,
    at                 timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (service_id, plugin_id, kind),
    CONSTRAINT plugin_downloads_plugin_id_not_empty CHECK (plugin_id <> '')
);

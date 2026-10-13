-- 0185_engine_images_explicit.sql
--
-- An engine whose spec names no image ran the DEFAULT of whichever egret
-- applied it last (`engines.rs`: the Trino `imageVersion` and StarRocks
-- `starrocksConfig.feImage` / `cnImage` fall back to `DEFAULT_ENGINE_IMAGES`).
-- The defaults move to Trino 483 and StarRocks 4.1.6 in the same change, so
-- without this every such engine would be upgraded on its next apply, behind
-- its operator's back.
-- Write the defaults those engines run TODAY into their rows: Trino 470, and
-- StarRocks' floating `4.1-latest` (which is exactly what they run; egret now
-- reports it as unmodelled until the operator pins it).
-- The StarRocks image is chosen by `starrocksConfig.feImage` / `cnImage` only
-- (`engines.rs`, the FE/CN image reads); no other field picks it.
-- External engines are untouched: egret never ran their image.

-- A spec, or a `starrocksConfig`, that is not a JSON object (JSON `null`, an
-- array, a scalar) is read by Rust as carrying nothing (`spec["x"]` is Null),
-- so such an engine runs the default too. Each is replaced by an object: never
-- `jsonb_set` into a non-object (it raises and aborts startup), and never
-- `'null'::jsonb || {...}` (that yields an array).

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(
                CASE WHEN jsonb_typeof(spec) = 'object' THEN spec ELSE '{}'::jsonb END,
                '{imageVersion}', '"470"')
 WHERE type = 'trino' AND origin = 'provisioned'
   AND coalesce(btrim(spec ->> 'imageVersion'), '') = '';

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(
                CASE WHEN jsonb_typeof(spec) = 'object' THEN spec ELSE '{}'::jsonb END,
                '{starrocksConfig}',
                CASE WHEN jsonb_typeof(spec -> 'starrocksConfig') = 'object'
                     THEN spec -> 'starrocksConfig' ELSE '{}'::jsonb END
                || jsonb_build_object('feImage', 'starrocks/fe-ubuntu:4.1-latest'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND coalesce(btrim(spec #>> '{starrocksConfig,feImage}'), '') = '';

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(
                CASE WHEN jsonb_typeof(spec) = 'object' THEN spec ELSE '{}'::jsonb END,
                '{starrocksConfig}',
                CASE WHEN jsonb_typeof(spec -> 'starrocksConfig') = 'object'
                     THEN spec -> 'starrocksConfig' ELSE '{}'::jsonb END
                || jsonb_build_object('cnImage', 'starrocks/cn-ubuntu:4.1-latest'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND coalesce(btrim(spec #>> '{starrocksConfig,cnImage}'), '') = '';

-- A non-string image (e.g. the JSON number 470) reads as '470' through `->>`, so
-- the statements above skip it, but the Rust side reads it with `as_str()` and
-- finds none. Normalise each to the JSON string it spells.
UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{imageVersion}', to_jsonb(spec ->> 'imageVersion'))
 WHERE type = 'trino' AND origin = 'provisioned'
   AND jsonb_typeof(spec -> 'imageVersion') NOT IN ('string', 'null');

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{starrocksConfig,feImage}',
                to_jsonb(spec #>> '{starrocksConfig,feImage}'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND jsonb_typeof(spec #> '{starrocksConfig,feImage}') NOT IN ('string', 'null');

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{starrocksConfig,cnImage}',
                to_jsonb(spec #>> '{starrocksConfig,cnImage}'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND jsonb_typeof(spec #> '{starrocksConfig,cnImage}') NOT IN ('string', 'null');

-- SP3b D1/D2: a StarRocks attachment exists in egret before the FE has been
-- told about it — an attach records first and converges after (the FE of a
-- freshly provisioned engine is unreachable for 5-10 minutes), and engine
-- create has always written the primary attachment the same way. The CHECK
-- from 0147 allowed only 'applied' and 'failed', so that row read 'applied'
-- while nothing had been applied. 'pending' is that state, said out loud.
ALTER TABLE registry.engine_catalog_attachments
    DROP CONSTRAINT engine_catalog_attachments_apply_status_check;
ALTER TABLE registry.engine_catalog_attachments
    ADD CONSTRAINT engine_catalog_attachments_apply_status_check
    CHECK (apply_status IN ('applied', 'failed', 'pending'));

-- BKR-41: durable business events and fenced asynchronous work.
-- This migration is additive. It does not dispatch a notification or change
-- existing chapter publication semantics.
CREATE TABLE outbox_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    aggregate_type TEXT NOT NULL CHECK (char_length(aggregate_type) BETWEEN 1 AND 80),
    aggregate_id BIGINT NOT NULL CHECK (aggregate_id > 0),
    event_type TEXT NOT NULL CHECK (char_length(event_type) BETWEEN 1 AND 120),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    dedup_key TEXT NOT NULL UNIQUE CHECK (char_length(dedup_key) BETWEEN 8 AND 160),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX outbox_events_aggregate_idx ON outbox_events(aggregate_type, aggregate_id, id);

-- Outbox payload is immutable; per-consumer delivery state is separate.
CREATE FUNCTION reject_outbox_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'outbox events are immutable' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER outbox_events_immutable
    BEFORE UPDATE OR DELETE ON outbox_events
    FOR EACH ROW EXECUTE FUNCTION reject_outbox_mutation();

CREATE TABLE outbox_deliveries (
    event_id BIGINT NOT NULL REFERENCES outbox_events(id) ON DELETE RESTRICT,
    consumer TEXT NOT NULL CHECK (char_length(consumer) BETWEEN 1 AND 80),
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, consumer)
);
CREATE INDEX outbox_deliveries_consumer_idx ON outbox_deliveries(consumer, delivered_at DESC);

CREATE TABLE durable_jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (char_length(kind) BETWEEN 1 AND 80),
    dedup_key TEXT NOT NULL CHECK (char_length(dedup_key) BETWEEN 1 AND 160),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','succeeded','dead','cancelled')),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'),
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    last_error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(kind, dedup_key),
    CONSTRAINT jobs_lease_state CHECK (
        (status = 'running' AND lease_owner IS NOT NULL AND lease_until IS NOT NULL)
        OR (status <> 'running' AND lease_owner IS NULL AND lease_until IS NULL)
    )
);
CREATE INDEX durable_jobs_ready_idx
    ON durable_jobs(run_after, id) WHERE status = 'queued';
CREATE INDEX durable_jobs_expired_leases_idx
    ON durable_jobs(lease_until, id) WHERE status = 'running';

-- Claim pattern:
-- SELECT id FROM durable_jobs WHERE status = 'queued' AND run_after <= now()
-- ORDER BY run_after,id FOR UPDATE SKIP LOCKED LIMIT $batch;
-- UPDATE ... SET status='running', lease_owner=$actor, lease_until=now()+$ttl,
-- lease_epoch=lease_epoch+1, attempts=attempts+1 ...
-- Completion/retry must match id, status='running', lease_owner and lease_epoch,
-- and a still-valid lease. A stale worker cannot finalize a newer claim.

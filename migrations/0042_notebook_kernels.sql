CREATE TABLE notebook_kernels (
    notebook_id  uuid NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kernel_id    text NOT NULL,                 -- the gateway's kernel id
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (notebook_id, user_id)
);

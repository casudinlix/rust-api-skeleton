CREATE TABLE users (
    id UUID PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    -- Emails are trimmed and lowercased by the application before every write.
    -- Bytewise comparison keeps uniqueness consistent with the MySQL schema.
    email VARCHAR(254) COLLATE "C" NOT NULL,
    password_hash TEXT NOT NULL,
    role VARCHAR(20) NOT NULL DEFAULT 'user',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT users_email_unique UNIQUE (email),
    CONSTRAINT users_role_check CHECK (role IN ('user', 'admin'))
);

CREATE INDEX users_created_at_id_index ON users (created_at DESC, id DESC);

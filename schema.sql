CREATE TABLE IF NOT EXISTS authentication (
    api_key VARCHAR(32),
    created_on DATETIME,
    active BOOLEAN DEFAULT TRUE,
    PRIMARY KEY (api_key)
);

CREATE TABLE IF NOT EXISTS upload (
    identifier VARCHAR(64),
    created_on DATETIME,
    api_key_used VARCHAR(32),
    PRIMARY KEY (identifier)
);
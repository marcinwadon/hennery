ALTER TABLE external_clients ADD COLUMN session_capacity INTEGER NOT NULL DEFAULT 32 CHECK(session_capacity BETWEEN 1 AND 1024);

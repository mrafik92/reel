-- Implicit single user until auth lands (TODO(auth)): API handlers use user id 1.
INSERT OR IGNORE INTO users (id, username, password_hash, role)
VALUES (1, 'admin', '!', 'admin');

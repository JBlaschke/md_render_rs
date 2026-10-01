```sql
-- comment
CREATE TABLE IF NOT EXISTS users (
    id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    created_at TIMESTAMP DEFAULT now()
);

INSERT INTO users (email) VALUES ('a@b.c'), ('x@y.z');

SELECT u.id, COUNT(o.id) AS orders, SUM(o.total) / 100.0
FROM users u
LEFT JOIN orders o ON o.user_id = u.id
WHERE u.created_at > '2024-01-01' AND u.email LIKE '%@example.com'
GROUP BY u.id
HAVING COUNT(*) > 1
ORDER BY orders DESC
LIMIT 10;

UPDATE users SET email = lower(email) WHERE id IN (SELECT id FROM banned);
/* block */
```

# Social-api-service

A high-performance Likes service built in Rust using **Axum**, **PostgreSQL**, and **Redis**.

This service supports:

- Idempotent like/unlike
- Redis-backed like counts
- Batch APIs (hot path optimized)
- Cursor-based pagination
- Leaderboard queries
- Health endpoints

---

# 🧱 Tech Stack

- Rust
- Axum (HTTP framework)
- Tokio (async runtime)
- SQLx (PostgreSQL driver)
- PostgreSQL (primary datastore)
- Redis (caching layer)

---

# 🏗 Architecture Overview

## Data Storage

PostgreSQL is the source of truth.

Each like record contains:

- `user_id`
- `content_type`
- `content_id`
- `liked_at`

Primary key ensures idempotency.

## Caching Strategy

Like counts are cached in Redis.

Key format:
likes:count:{content_type}:{content_id}

Strategy:

- Read → Redis first
- Cache miss → DB fallback
- TTL: 5 minutes
- Like → INCR
- Unlike → DECR

---

# 📦 Database Schema

```sql
CREATE TABLE likes (
    user_id UUID NOT NULL,
    content_type TEXT NOT NULL,
    content_id UUID NOT NULL,
    liked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, content_type, content_id)
);

CREATE INDEX idx_content_lookup
ON likes (content_type, content_id);

CREATE INDEX idx_user_lookup
ON likes (user_id, liked_at DESC);

```

Why These Indexes?

(user_id, content_type, content_id) → idempotent writes

(content_type, content_id) → fast count queries

(user_id, liked_at DESC) → efficient cursor pagination




# 📡 API Endpoints

Base URL:

http://localhost:8080


---

## 1️⃣ Like Content

POST /v1/likes

Headers:
X-User-Id: <uuid>

Body:
{
  "content_type": "post",
  "content_id": "uuid"
}

Behavior:
- Idempotent
- Returns whether already existed
- Updates Redis count

---

## 2️⃣ Unlike Content

DELETE /v1/likes/{content_type}/{content_id}

Headers:
X-User-Id: <uuid>

Behavior:
- Idempotent
- Decrements Redis count if existed

---

## 3️⃣ Get Like Count

GET /v1/likes/{content_type}/{content_id}/count

Behavior:
- Redis-first
- DB fallback
- TTL: 5 minutes

---

## 4️⃣ Batch Get Like Counts

POST /v1/likes/batch/counts

Body:
{
  "items": [
    { "content_type": "post", "content_id": "uuid1" },
    { "content_type": "post", "content_id": "uuid2" }
  ]
}

Constraints:
- Max 100 items
- Uses Redis MGET
- DB fallback for cache misses

---

## 5️⃣ Batch Get Like Statuses

POST /v1/likes/batch/statuses

Headers:
X-User-Id: <uuid>

Body:
{
  "items": [
    { "content_type": "post", "content_id": "uuid1" },
    { "content_type": "post", "content_id": "uuid2" }
  ]
}

Behavior:
- Single DB query using ANY()
- Returns liked + liked_at per item

---

## 6️⃣ Get User Likes (Cursor Pagination)

GET /v1/likes/user?limit=20&cursor=<base64>&content_type=post

Headers:
X-User-Id: <uuid>

Query Params:
- limit (default 20, max 50)
- cursor (base64 encoded timestamp)
- content_type (optional filter)

Behavior:
- Ordered by liked_at DESC
- Returns:
  - items
  - next_cursor
  - has_more

---

## 7️⃣ Get Top Liked Content

GET /v1/likes/top?window=7d&limit=10&content_type=post

Query Params:
- window = 24h | 7d | 30d | all
- limit (default 10, max 50)
- content_type (optional)

Behavior:
- Aggregates by content_type + content_id
- Orders by count DESC

---

## 8️⃣ Health Checks

GET /health/live
GET /health/ready

Purpose:
- Liveness probe
- Readiness probe (DB + Redis check)

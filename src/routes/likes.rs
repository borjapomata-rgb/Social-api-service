use axum::{
    routing::{post, delete, get},
    Router,
    Json,
    extract::{State, Path, Query},
    http::{StatusCode, HeaderMap},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use redis::AsyncCommands;
use std::collections::HashMap;
use base64::{engine::general_purpose, Engine as _};
use sqlx::Row;
use chrono::{DateTime, Utc};


use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/likes", post(like_content))
        .route("/v1/likes/:content_type/:content_id", delete(unlike_content))
        .route(
            "/v1/likes/:content_type/:content_id/count",
            get(get_like_count),
        )
        .route("/v1/likes/batch/counts", post(batch_get_counts))
        .route("/v1/likes/batch/statuses", post(batch_get_statuses))
        .route("/v1/likes/user", get(get_user_likes))
        .route("/v1/likes/top", get(get_top_liked))
}

#[derive(Deserialize)]
pub struct LikeRequest {
    pub content_type: String,
    pub content_id: String,
}

#[derive(Serialize)]
pub struct LikeResponse {
    pub liked: bool,
    pub already_existed: bool,
    pub count: i64,
    pub liked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
pub struct UnlikeResponse {
    pub liked: bool,
    pub was_liked: bool,
    pub count: i64,
}

#[derive(Serialize)]
pub struct CountResponse {
    pub content_type: String,
    pub content_id: String,
    pub count: i64,
}

#[derive(Deserialize)]
pub struct BatchCountRequest {
    pub items: Vec<BatchItem>,
}

#[derive(Deserialize, Clone)]
pub struct BatchItem {
    pub content_type: String,
    pub content_id: String,
}

#[derive(Serialize)]
pub struct BatchCountResponse {
    pub results: Vec<BatchCountItem>,
}

#[derive(Serialize)]
pub struct BatchCountItem {
    pub content_type: String,
    pub content_id: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct BatchStatusResponse {
    pub results: Vec<BatchStatusItem>,
}

#[derive(Serialize)]
pub struct BatchStatusItem {
    pub content_type: String,
    pub content_id: String,
    pub liked: bool,
    pub liked_at: Option<chrono::DateTime<chrono::Utc>>,
}


#[derive(Serialize)]
pub struct UserLikesResponse {
    pub items: Vec<UserLikeItem>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct UserLikeItem {
    pub content_type: String,
    pub content_id: String,
    pub liked_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct TopLikedResponse {
    pub window: String,
    pub content_type: Option<String>,
    pub items: Vec<TopLikedItem>,
}

#[derive(Serialize)]
pub struct TopLikedItem {
    pub content_type: String,
    pub content_id: String,
    pub count: i64,
}


fn extract_user_id(headers: &HeaderMap) -> Result<Uuid, (StatusCode, String)> {
    let user_id_header = headers
        .get("x-user-id")
        .ok_or((StatusCode::UNAUTHORIZED, "Missing X-User-Id header".to_string()))?
        .to_str()
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid X-User-Id header".to_string()))?;

    Uuid::parse_str(user_id_header)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid user_id".to_string()))
}



async fn like_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LikeRequest>,
) -> Result<(StatusCode, Json<LikeResponse>), (StatusCode, String)> {

    let user_id = extract_user_id(&headers)?;

    let content_uuid = Uuid::parse_str(&payload.content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

    // Insert idempotent
    let inserted = sqlx::query!(
        r#"
        INSERT INTO likes (user_id, content_type, content_id)
        VALUES ($1, $2, $3)
        ON CONFLICT DO NOTHING
        RETURNING liked_at
        "#,
        user_id,
        payload.content_type,
        content_uuid
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    let (already_existed, liked_at) = match inserted {
        Some(row) => (false, Some(row.liked_at)),
        None => {
            let existing = sqlx::query!(
                r#"
                SELECT liked_at
                FROM likes
                WHERE user_id = $1
                  AND content_type = $2
                  AND content_id = $3
                "#,
                user_id,
                payload.content_type,
                content_uuid
            )
            .fetch_one(&state.db)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

            (true, Some(existing.liked_at))
        }
    };

    // Get count
    let count_row = sqlx::query!(
        r#"
        SELECT COUNT(*) as count
        FROM likes
        WHERE content_type = $1
          AND content_id = $2
        "#,
        payload.content_type,
        content_uuid
    )
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    let count = count_row.count.unwrap_or(0);

    // Update Redis only if new like
    if !already_existed {
        let cache_key = format!("likes:count:{}:{}", payload.content_type, payload.content_id);
        if let Ok(mut conn) = state.redis.get_async_connection().await {
            let _: Result<i64, _> = conn.incr(&cache_key, 1).await;
        }
    }

    Ok((
        StatusCode::CREATED,
        Json(LikeResponse {
            liked: true,
            already_existed,
            count,
            liked_at,
        }),
    ))
}


async fn unlike_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((content_type, content_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<UnlikeResponse>), (StatusCode, String)> {

    let user_id = extract_user_id(&headers)?;

    let content_uuid = Uuid::parse_str(&content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

    let result = sqlx::query!(
        r#"
        DELETE FROM likes
        WHERE user_id = $1
          AND content_type = $2
          AND content_id = $3
        RETURNING liked_at
        "#,
        user_id,
        content_type,
        content_uuid
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    let was_liked = result.is_some();

    let count_row = sqlx::query!(
        r#"
        SELECT COUNT(*) as count
        FROM likes
        WHERE content_type = $1
          AND content_id = $2
        "#,
        content_type,
        content_uuid
    )
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    let count = count_row.count.unwrap_or(0);

    if was_liked {
        let cache_key = format!("likes:count:{}:{}", content_type, content_id);
        if let Ok(mut conn) = state.redis.get_async_connection().await {
            let _: Result<i64, _> = conn.decr(&cache_key, 1).await;
        }
    }

    Ok((
        StatusCode::OK,
        Json(UnlikeResponse {
            liked: false,
            was_liked,
            count,
        }),
    ))
}


async fn get_like_count(
    State(state): State<AppState>,
    Path((content_type, content_id)): Path<(String, String)>,
) -> Result<Json<CountResponse>, (StatusCode, String)> {

    let content_uuid = Uuid::parse_str(&content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

    let cache_key = format!("likes:count:{}:{}", content_type, content_id);

    // 🔥 1. Try Redis first (fast path)
    if let Ok(mut conn) = state.redis.get_async_connection().await {
        if let Ok(Some(cached)) = conn.get::<_, Option<i64>>(&cache_key).await {
            return Ok(Json(CountResponse {
                content_type,
                content_id,
                count: cached,
            }));
        }

        // 🔥 2. Cache miss → fetch from DB
        let row = sqlx::query!(
            r#"
            SELECT COUNT(*) as count
            FROM likes
            WHERE content_type = $1
              AND content_id = $2
            "#,
            content_type,
            content_uuid
        )
        .fetch_one(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

        let count = row.count.unwrap_or(0);

        // 🔥 3. Store in Redis (TTL 5 min)
        let _: Result<(), _> = conn.set_ex(&cache_key, count, 300).await;

        return Ok(Json(CountResponse {
            content_type,
            content_id,
            count,
        }));
    }

    // 🔥 4. Redis unavailable → fallback to DB
    let row = sqlx::query!(
        r#"
        SELECT COUNT(*) as count
        FROM likes
        WHERE content_type = $1
          AND content_id = $2
        "#,
        content_type,
        content_uuid
    )
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    Ok(Json(CountResponse {
        content_type,
        content_id,
        count: row.count.unwrap_or(0),
    }))
}


async fn batch_get_counts(
    State(state): State<AppState>,
    Json(payload): Json<BatchCountRequest>,
) -> Result<Json<BatchCountResponse>, (StatusCode, String)> {

    if payload.items.len() > 100 {
        return Err((StatusCode::BAD_REQUEST, "BATCH_TOO_LARGE".to_string()));
    }

    let mut results = Vec::with_capacity(payload.items.len());
    let mut missing_indices = Vec::new();

    let keys: Vec<String> = payload.items.iter()
        .map(|item| format!("likes:count:{}:{}", item.content_type, item.content_id))
        .collect();

    // 🔥 Try Redis MGET
    if let Ok(mut conn) = state.redis.get_async_connection().await {

        let cached: Vec<Option<i64>> =
            conn.get(keys.clone()).await.unwrap_or_default();

        for (i, value) in cached.iter().enumerate() {
            if let Some(count) = value {
                results.push(BatchCountItem {
                    content_type: payload.items[i].content_type.clone(),
                    content_id: payload.items[i].content_id.clone(),
                    count: *count,
                });
            } else {
                missing_indices.push(i);
            }
        }

        // 🔥 Fetch missing from DB
        if !missing_indices.is_empty() {

            for i in missing_indices {

                let item = &payload.items[i];

                let uuid = Uuid::parse_str(&item.content_id)
                    .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

                let row = sqlx::query!(
                    r#"
                    SELECT COUNT(*) as count
                    FROM likes
                    WHERE content_type = $1
                      AND content_id = $2
                    "#,
                    item.content_type,
                    uuid
                )
                .fetch_one(&state.db)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

                let count = row.count.unwrap_or(0);

                // Update cache
                let key = format!("likes:count:{}:{}", item.content_type, item.content_id);
                let _: Result<(), _> = conn.set_ex(&key, count, 300).await;

                results.push(BatchCountItem {
                    content_type: item.content_type.clone(),
                    content_id: item.content_id.clone(),
                    count,
                });
            }
        }

        return Ok(Json(BatchCountResponse { results }));
    }

    // 🔥 Redis unavailable → DB only
    for item in payload.items {

        let uuid = Uuid::parse_str(&item.content_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

        let row = sqlx::query!(
            r#"
            SELECT COUNT(*) as count
            FROM likes
            WHERE content_type = $1
              AND content_id = $2
            "#,
            item.content_type,
            uuid
        )
        .fetch_one(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

        results.push(BatchCountItem {
            content_type: item.content_type,
            content_id: item.content_id,
            count: row.count.unwrap_or(0),
        });
    }

    Ok(Json(BatchCountResponse { results }))
}


async fn batch_get_statuses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<BatchCountRequest>,
) -> Result<Json<BatchStatusResponse>, (StatusCode, String)> {

    if payload.items.len() > 100 {
        return Err((StatusCode::BAD_REQUEST, "BATCH_TOO_LARGE".to_string()));
    }

    let user_id = extract_user_id(&headers)?;

    // Validate and collect UUIDs
    let mut content_pairs = Vec::new();

    for item in &payload.items {
        let uuid = Uuid::parse_str(&item.content_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

        content_pairs.push((item.content_type.clone(), uuid));
    }

    if content_pairs.is_empty() {
        return Ok(Json(BatchStatusResponse { results: vec![] }));
    }

    // 🔥 Build dynamic query using ANY
    // We query all likes for user where content_id in list
    let content_ids: Vec<Uuid> = content_pairs.iter().map(|(_, id)| *id).collect();

    let rows = sqlx::query!(
        r#"
        SELECT content_type, content_id, liked_at
        FROM likes
        WHERE user_id = $1
          AND content_id = ANY($2)
        "#,
        user_id,
        &content_ids
    )
    .fetch_all(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    // Map results
    let mut liked_map: HashMap<(String, Uuid), DateTime<Utc>> = HashMap::new();

    for row in rows {
        liked_map.insert(
            (row.content_type.clone(), row.content_id),
            row.liked_at,
        );
    }

    let mut results = Vec::with_capacity(payload.items.len());

    for item in payload.items {
        let uuid = Uuid::parse_str(&item.content_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CONTENT_ID".to_string()))?;

        if let Some(ts) = liked_map.get(&(item.content_type.clone(), uuid)) {
            results.push(BatchStatusItem {
                content_type: item.content_type,
                content_id: item.content_id,
                liked: true,
                liked_at: Some(*ts),
            });
        } else {
            results.push(BatchStatusItem {
                content_type: item.content_type,
                content_id: item.content_id,
                liked: false,
                liked_at: None,
            });
        }
    }

    Ok(Json(BatchStatusResponse { results }))
}



async fn get_user_likes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<UserLikesResponse>, (StatusCode, String)> {

    let user_id = extract_user_id(&headers)?;

    let limit: i64 = params.get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(20)
        .min(50);

    let content_type_filter = params.get("content_type");
    let cursor_param = params.get("cursor");

    let mut query = String::from(
        "SELECT content_type, content_id, liked_at
         FROM likes
         WHERE user_id = $1"
    );

    let mut bind_index = 2;

    if content_type_filter.is_some() {
        query.push_str(&format!(" AND content_type = ${}", bind_index));
        bind_index += 1;
    }

    if cursor_param.is_some() {
        query.push_str(&format!(" AND liked_at < ${}", bind_index));
        bind_index += 1;
    }

    query.push_str(" ORDER BY liked_at DESC");
    query.push_str(&format!(" LIMIT {}", limit + 1));

    let mut q = sqlx::query(&query).bind(user_id);

    if let Some(ct) = content_type_filter {
        q = q.bind(ct);
    }

    if let Some(cursor_value) = cursor_param {
        let decoded = general_purpose::STANDARD
            .decode(cursor_value)
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CURSOR".to_string()))?;

        let cursor_str = String::from_utf8(decoded)
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CURSOR".to_string()))?;

        let ts: DateTime<Utc> = cursor_str.parse()
            .map_err(|_| (StatusCode::BAD_REQUEST, "INVALID_CURSOR".to_string()))?;

        q = q.bind(ts);
    }

    let rows = q
        .fetch_all(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

    let has_more = rows.len() as i64 > limit;

    let sliced = if has_more {
        &rows[..limit as usize]
    } else {
        &rows[..]
    };

    let mut items = Vec::new();

    for row in sliced {
        let content_type: String = row.try_get("content_type").unwrap();
        let content_id: Uuid = row.try_get("content_id").unwrap();
        let liked_at: DateTime<Utc> = row.try_get("liked_at").unwrap();

        items.push(UserLikeItem {
            content_type,
            content_id: content_id.to_string(),
            liked_at,
        });
    }

    let next_cursor = if has_more {
        let last = sliced.last().unwrap();
        let ts: DateTime<Utc> = last.try_get("liked_at").unwrap();
        Some(general_purpose::STANDARD.encode(ts.to_string()))
    } else {
        None
    };

    Ok(Json(UserLikesResponse {
        items,
        next_cursor,
        has_more,
    }))
}


async fn get_top_liked(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<TopLikedResponse>, (StatusCode, String)> {

    let window = params.get("window").cloned().unwrap_or("all".to_string());

    let limit: i64 = params.get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(10)
        .min(50);

    let content_type_filter = params.get("content_type");

    // Determine time window
    let interval_clause = match window.as_str() {
        "24h" => Some("NOW() - INTERVAL '24 hours'"),
        "7d" => Some("NOW() - INTERVAL '7 days'"),
        "30d" => Some("NOW() - INTERVAL '30 days'"),
        "all" => None,
        _ => return Err((StatusCode::BAD_REQUEST, "INVALID_WINDOW".to_string())),
    };

    let mut query = String::from(
        "SELECT content_type, content_id, COUNT(*) as count
         FROM likes"
    );

    let mut conditions = Vec::new();
    let mut bind_index = 1;

    if let Some(interval) = interval_clause {
        conditions.push(format!("liked_at >= {}", interval));
    }

    if content_type_filter.is_some() {
        conditions.push(format!("content_type = ${}", bind_index));
        bind_index += 1;
    }

    if !conditions.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&conditions.join(" AND "));
    }

    query.push_str(" GROUP BY content_type, content_id");
    query.push_str(" ORDER BY count DESC");
    query.push_str(&format!(" LIMIT {}", limit));

    let rows = if let Some(ct) = content_type_filter {
        sqlx::query(&query)
            .bind(ct)
            .fetch_all(&state.db)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?
    } else {
        sqlx::query(&query)
            .fetch_all(&state.db)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?
    };

    let mut items = Vec::new();

    for row in rows {
        let content_type: String = row.try_get("content_type").unwrap();
        let content_id: Uuid = row.try_get("content_id").unwrap();
        let count: i64 = row.try_get("count").unwrap();

        items.push(TopLikedItem {
            content_type,
            content_id: content_id.to_string(),
            count,
        });
    }

    Ok(Json(TopLikedResponse {
        window,
        content_type: content_type_filter.cloned(),
        items,
    }))
}



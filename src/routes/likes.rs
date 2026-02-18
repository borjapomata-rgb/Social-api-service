use axum::{
    routing::{post, delete, get},
    Router,
    Json,
    extract::{State, Path},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use redis::AsyncCommands;

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


async fn like_content(
    State(state): State<AppState>,
    Json(payload): Json<LikeRequest>,
) -> Result<(StatusCode, Json<LikeResponse>), (StatusCode, String)> {

    // TEMP hardcoded user_id
    let user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid user_id".to_string()))?;

    let content_uuid = Uuid::parse_str(&payload.content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

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

    let already_existed;
    let liked_at;

    match inserted {
        Some(row) => {
            already_existed = false;
            liked_at = Some(row.liked_at);
        }
        None => {
            already_existed = true;

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

            liked_at = Some(existing.liked_at);
        }
    }

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

    // 🔥 Update Redis if new like
    if !already_existed {
        let cache_key = format!("likes:count:{}:{}", payload.content_type, payload.content_id);
        if let Ok(mut conn) = state.redis.get_async_connection().await {
            let _: Result<i64, _> = conn.incr(&cache_key, 1).await;
        }
    }

    let response = LikeResponse {
        liked: true,
        already_existed,
        count,
        liked_at,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

async fn unlike_content(
    State(state): State<AppState>,
    Path((content_type, content_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<UnlikeResponse>), (StatusCode, String)> {

    let user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid user_id".to_string()))?;

    let content_uuid = Uuid::parse_str(&content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

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

    // 🔥 Update Redis if actually deleted
    if was_liked {
        let cache_key = format!("likes:count:{}:{}", content_type, content_id);
        if let Ok(mut conn) = state.redis.get_async_connection().await {
            let _: Result<i64, _> = conn.decr(&cache_key, 1).await;
        }
    }

    let response = UnlikeResponse {
        liked: false,
        was_liked,
        count,
    };

    Ok((StatusCode::OK, Json(response)))
}

async fn get_like_count(
    State(state): State<AppState>,
    Path((content_type, content_id)): Path<(String, String)>,
) -> Result<Json<CountResponse>, (StatusCode, String)> {

    let content_uuid = Uuid::parse_str(&content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

    let cache_key = format!("likes:count:{}:{}", content_type, content_id);

    if let Ok(mut conn) = state.redis.get_async_connection().await {

        // Try cache
        if let Ok(cached) = conn.get::<_, i64>(&cache_key).await {
            let response = CountResponse {
                content_type,
                content_id,
                count: cached,
            };
            return Ok(Json(response));
        }

        // Fallback to DB
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

        // Store in cache (5 min TTL)
        let _: Result<(), _> = conn.set_ex(&cache_key, count, 300).await;

        let response = CountResponse {
            content_type,
            content_id,
            count,
        };

        return Ok(Json(response));
    }

    // If Redis completely unavailable → DB only
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

    Ok(Json(CountResponse {
        content_type,
        content_id,
        count: count_row.count.unwrap_or(0),
    }))
}

async fn batch_get_counts(
    State(state): State<AppState>,
    Json(payload): Json<BatchCountRequest>,
) -> Result<Json<BatchCountResponse>, (StatusCode, String)> {

    if payload.items.len() > 100 {
        return Err((StatusCode::BAD_REQUEST, "BATCH_TOO_LARGE".to_string()));
    }

    let mut results = Vec::new();
    let mut missing = Vec::new();

    // Build cache keys
    let keys: Vec<String> = payload.items.iter()
        .map(|item| format!("likes:count:{}:{}", item.content_type, item.content_id))
        .collect();

    // Try Redis MGET
    if let Ok(mut conn) = state.redis.get_async_connection().await {
        let cached: Vec<Option<i64>> = conn.get(keys.clone()).await.unwrap_or_default();

        for (i, value) in cached.iter().enumerate() {
            if let Some(count) = value {
                results.push(BatchCountItem {
                    content_type: payload.items[i].content_type.clone(),
                    content_id: payload.items[i].content_id.clone(),
                    count: *count,
                });
            } else {
                missing.push(payload.items[i].clone());
            }
        }

        // Fetch missing from DB in ONE query
        if !missing.is_empty() {

            let mut db_results = Vec::new();

            for item in &missing {
                let uuid = Uuid::parse_str(&item.content_id)
                    .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

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

                db_results.push((item.clone(), count));
            }

            // Update cache
            for (item, count) in db_results {
                let key = format!("likes:count:{}:{}", item.content_type, item.content_id);

                let _: Result<(), _> = conn.set_ex(&key, count, 300).await;

                results.push(BatchCountItem {
                    content_type: item.content_type,
                    content_id: item.content_id,
                    count,
                });
            }
        }

        return Ok(Json(BatchCountResponse { results }));
    }

    // If Redis unavailable → DB only
    for item in payload.items {
        let uuid = Uuid::parse_str(&item.content_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

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
    Json(payload): Json<BatchCountRequest>, // reuse same request struct
) -> Result<Json<BatchStatusResponse>, (StatusCode, String)> {

    if payload.items.len() > 100 {
        return Err((StatusCode::BAD_REQUEST, "BATCH_TOO_LARGE".to_string()));
    }

    // TEMP hardcoded user_id
    let user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid user_id".to_string()))?;

    let mut results = Vec::new();

    for item in payload.items {

        let uuid = Uuid::parse_str(&item.content_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

        let row = sqlx::query!(
            r#"
            SELECT liked_at
            FROM likes
            WHERE user_id = $1
              AND content_type = $2
              AND content_id = $3
            "#,
            user_id,
            item.content_type,
            uuid
        )
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string()))?;

        match row {
            Some(r) => {
                results.push(BatchStatusItem {
                    content_type: item.content_type,
                    content_id: item.content_id,
                    liked: true,
                    liked_at: Some(r.liked_at),
                });
            }
            None => {
                results.push(BatchStatusItem {
                    content_type: item.content_type,
                    content_id: item.content_id,
                    liked: false,
                    liked_at: None,
                });
            }
        }
    }

    Ok(Json(BatchStatusResponse { results }))
}

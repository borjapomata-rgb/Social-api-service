use axum::{
    routing::post,
    Router,
    Json,
    extract::State,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/likes", post(like_content))
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

async fn like_content(
    State(state): State<AppState>,
    Json(payload): Json<LikeRequest>,
) -> Result<(StatusCode, Json<LikeResponse>), (StatusCode, String)> {

    // TEMP: hardcoded user_id for now
    let user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid user_id".to_string()))?;

    // Validate content_id
    let content_uuid = Uuid::parse_str(&payload.content_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid content_id".to_string()))?;

    // Insert like (idempotent)
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
    .map_err(|e| {
        eprintln!("DB error: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string())
    })?;

    let already_existed;
    let liked_at;

    match inserted {
        Some(row) => {
            already_existed = false;
            liked_at = Some(row.liked_at);
        }
        None => {
            already_existed = true;

            // Fetch existing liked_at
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

    // Count total likes
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

    let response = LikeResponse {
        liked: true,
        already_existed,
        count: count_row.count.unwrap_or(0),
        liked_at,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

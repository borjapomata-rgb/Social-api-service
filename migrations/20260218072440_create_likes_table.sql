CREATE TABLE likes (
    user_id UUID NOT NULL,
    content_type TEXT NOT NULL,
    content_id UUID NOT NULL,
    liked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, content_type, content_id)
);

CREATE INDEX idx_likes_content 
ON likes (content_type, content_id);

CREATE INDEX idx_likes_user_liked_at
ON likes (user_id, liked_at DESC);

CREATE INDEX idx_likes_liked_at
ON likes (liked_at DESC);

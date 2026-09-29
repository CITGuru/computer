use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

pub async fn offered(request: Request, next: Next) -> Response {
    #[cfg(feature = "vercel")]
    {
        use computer::sandboxes::vercel::oidc;

        if let Some(token) = request
            .headers()
            .get(oidc::HEADER)
            .and_then(|value| value.to_str().ok())
            && oidc::offer(token)
        {
            tracing::debug!("took a newer vercel oidc token");
        }
    }

    next.run(request).await
}

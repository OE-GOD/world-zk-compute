//! Swagger UI endpoint — serves interactive API explorer.
//!
//! `GET /docs` returns an HTML page with Swagger UI pointing at the OpenAPI spec.
//! `GET /docs/openapi.yaml` returns the raw OpenAPI specification.
//!
//! Both endpoints are public (no auth required).

/// Serve the Swagger UI HTML page.
pub async fn swagger_ui() -> axum::response::Html<String> {
    axum::response::Html(r#"<!DOCTYPE html>
<html><head><title>World ZK Compute API</title>
<link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist/swagger-ui.css">
</head><body>
<div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist/swagger-ui-bundle.js"></script>
<script>SwaggerUIBundle({url: '/docs/openapi.yaml', dom_id: '#swagger-ui'})</script>
</body></html>"#.into())
}

/// Serve the raw OpenAPI specification.
pub async fn openapi_spec() -> String {
    include_str!("../../verifier/openapi.yaml").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_swagger_ui_contains_swagger_elements() {
        let html = swagger_ui().await;
        let body = html.0;
        assert!(body.contains("swagger-ui"));
        assert!(body.contains("/docs/openapi.yaml"));
        assert!(body.contains("SwaggerUIBundle"));
    }

    #[tokio::test]
    async fn test_openapi_spec_returns_yaml() {
        let spec = openapi_spec().await;
        assert!(spec.contains("openapi:"));
        assert!(spec.contains("World ZK Compute"));
    }
}

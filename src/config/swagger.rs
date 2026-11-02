use anyhow::ensure;
use url::Url;

#[derive(Clone, Debug)]
pub struct SwaggerConfig {
    pub enabled: bool,
    pub ui_path: String,
    pub openapi_path: String,
    pub asset_base_url: String,
}

impl SwaggerConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        validate_route_path(&self.ui_path, "SWAGGER_UI_PATH")?;
        validate_route_path(&self.openapi_path, "SWAGGER_OPENAPI_PATH")?;
        ensure!(
            self.ui_path != self.openapi_path,
            "SWAGGER_UI_PATH and SWAGGER_OPENAPI_PATH must be different"
        );
        for reserved in ["/health", "/ready", "/api/v1"] {
            ensure!(
                self.ui_path != reserved
                    && self.openapi_path != reserved
                    && !self.ui_path.starts_with(&format!("{reserved}/"))
                    && !self.openapi_path.starts_with(&format!("{reserved}/")),
                "Swagger routes must not overlap health/readiness or /api/v1 business routes"
            );
        }

        let assets = Url::parse(&self.asset_base_url)
            .map_err(|_| anyhow::anyhow!("SWAGGER_ASSET_BASE_URL must be a valid HTTPS URL"))?;
        ensure!(
            assets.scheme() == "https"
                && assets.host_str().is_some()
                && assets.username().is_empty()
                && assets.password().is_none()
                && assets.query().is_none()
                && assets.fragment().is_none(),
            "SWAGGER_ASSET_BASE_URL must be an HTTPS URL without credentials, query, or fragment"
        );
        ensure!(
            !self
                .asset_base_url
                .chars()
                .any(|character| matches!(character, '<' | '>' | '"' | '\'')),
            "SWAGGER_ASSET_BASE_URL contains unsafe characters"
        );
        Ok(())
    }
}

fn validate_route_path(path: &str, name: &str) -> anyhow::Result<()> {
    ensure!(
        (2..=160).contains(&path.len())
            && path.starts_with('/')
            && !path.ends_with('/')
            && !path.contains("//")
            && path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.')),
        "{name} must be an absolute route path using only ASCII letters, numbers, '/', '-', '_' or '.', without a trailing slash"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SwaggerConfig {
        SwaggerConfig {
            enabled: false,
            ui_path: "/docs".into(),
            openapi_path: "/api-docs/openapi.json".into(),
            asset_base_url: "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.32.6".into(),
        }
    }

    #[test]
    fn disabled_swagger_ignores_unused_ui_settings() {
        let mut disabled = config();
        disabled.ui_path = "../unused".into();
        disabled.asset_base_url.clear();
        disabled.validate().unwrap();
    }

    #[test]
    fn enabled_swagger_validates_safe_non_overlapping_routes_and_https_assets() {
        let mut enabled = config();
        enabled.enabled = true;
        enabled.validate().unwrap();

        let mut invalid = enabled.clone();
        invalid.ui_path = "/api/v1/docs".into();
        assert!(invalid.validate().is_err());

        let mut invalid = enabled.clone();
        invalid.openapi_path = "../openapi.json".into();
        assert!(invalid.validate().is_err());

        let mut invalid = enabled;
        invalid.asset_base_url = "http://cdn.example/swagger".into();
        assert!(invalid.validate().is_err());
    }
}

use super::{RuntimeResult, runtime_error};

pub(super) fn normalize_image_reference(image: &str) -> RuntimeResult<String> {
    let image = image.trim();
    if image.is_empty() {
        return Err(runtime_error("image reference cannot be empty"));
    }
    if image.chars().any(char::is_whitespace) {
        return Err(runtime_error("image reference cannot contain whitespace"));
    }

    let first_component = image.split('/').next().unwrap_or_default();
    let has_registry = image.contains('/')
        && (first_component.contains('.')
            || first_component.contains(':')
            || first_component == "localhost");

    let mut normalized = if has_registry {
        image.to_string()
    } else {
        format!("docker.io/{image}")
    };

    if let Some(remainder) = normalized.strip_prefix("docker.io/") {
        if !remainder.contains('/') {
            normalized = format!("docker.io/library/{remainder}");
        }
    }

    if !has_tag_or_digest(&normalized) {
        normalized.push_str(":latest");
    }

    Ok(normalized)
}

fn has_tag_or_digest(reference: &str) -> bool {
    if reference.contains('@') {
        return true;
    }

    reference
        .rsplit('/')
        .next()
        .is_some_and(|last_component| last_component.contains(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_short_docker_hub_references() {
        assert_eq!(
            normalize_image_reference("hello-world").unwrap(),
            "docker.io/library/hello-world:latest"
        );
        assert_eq!(
            normalize_image_reference("library/alpine").unwrap(),
            "docker.io/library/alpine:latest"
        );
        assert_eq!(
            normalize_image_reference("alpine:3.20").unwrap(),
            "docker.io/library/alpine:3.20"
        );
    }

    #[test]
    fn preserves_registry_and_digest_references() {
        assert_eq!(
            normalize_image_reference("ghcr.io/acme/app").unwrap(),
            "ghcr.io/acme/app:latest"
        );
        assert_eq!(
            normalize_image_reference("localhost:5000/acme/app:v1").unwrap(),
            "localhost:5000/acme/app:v1"
        );
        assert_eq!(
            normalize_image_reference("alpine@sha256:abc").unwrap(),
            "docker.io/library/alpine@sha256:abc"
        );
    }

    #[test]
    fn rejects_invalid_references() {
        assert!(normalize_image_reference("").is_err());
        assert!(normalize_image_reference("hello world").is_err());
    }
}

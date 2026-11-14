use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServiceError;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub service_version: String,
}

pub fn write_endpoint(path: &Path, endpoint: &Endpoint) -> Result<(), ServiceError> {
    if endpoint.port == 0
        || endpoint.token.trim().is_empty()
        || endpoint.service_version.trim().is_empty()
    {
        return Err(endpoint_error("endpoint fields are invalid"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| endpoint_error("endpoint path has no parent"))?;
    fs::create_dir_all(parent).map_err(|_| endpoint_error("cannot create endpoint directory"))?;
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let bytes =
        serde_json::to_vec(endpoint).map_err(|_| endpoint_error("cannot encode endpoint"))?;
    fs::write(&temporary, bytes).map_err(|_| endpoint_error("cannot write endpoint"))?;
    set_private_permissions(&temporary)?;
    replace_file(&temporary, path).map_err(|_| endpoint_error("cannot publish endpoint"))
}

pub fn read_endpoint(path: &Path) -> Result<Endpoint, ServiceError> {
    let bytes = fs::read(path).map_err(|_| {
        ServiceError::new(
            "CLIPBOARD_SERVICE_ENDPOINT_READ_FAILED",
            "cannot read service endpoint",
        )
    })?;
    let endpoint: Endpoint =
        serde_json::from_slice(&bytes).map_err(|_| endpoint_error("cannot decode endpoint"))?;
    if endpoint.port == 0
        || endpoint.token.trim().is_empty()
        || endpoint.service_version.trim().is_empty()
    {
        return Err(endpoint_error("endpoint fields are invalid"));
    }
    Ok(endpoint)
}

fn endpoint_error(message: impl Into<String>) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_ENDPOINT_INVALID", message)
}

fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)
}

fn set_private_permissions(path: &Path) -> Result<(), ServiceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|_| endpoint_error("cannot inspect endpoint"))?
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)
            .map_err(|_| endpoint_error("cannot protect endpoint"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{read_endpoint, write_endpoint, Endpoint};

    #[test]
    fn publishes_a_new_endpoint_over_an_existing_one() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("service.endpoint.json");
        write_endpoint(
            &path,
            &Endpoint {
                port: 1001,
                token: "old".into(),
                service_version: "0.1.0".into(),
            },
        )
        .unwrap();

        write_endpoint(
            &path,
            &Endpoint {
                port: 1002,
                token: "new".into(),
                service_version: "0.1.1".into(),
            },
        )
        .unwrap();

        assert_eq!(read_endpoint(&path).unwrap().token, "new");
    }
}

//! OS-backed environment and Auth secrets, in separate namespaces scoped by workspace
//! and owner UUID. Reference values never identify a credential in another namespace.

use anyhow::Result;
use uuid::Uuid;

#[cfg(not(test))]
fn entry(
    domain: &str,
    workspace: Uuid,
    environment: Uuid,
    variable: Uuid,
) -> Result<keyring::Entry> {
    keyring::Entry::new(
        &format!("com.apogee.workspace.{workspace}.{domain}.{environment}"),
        &variable.to_string(),
    )
    .map_err(|_| anyhow::anyhow!("无法访问系统凭据存储"))
}

#[cfg(test)]
pub(crate) fn test_references(workspace: Uuid, environment: Uuid) -> Result<Vec<Uuid>> {
    test_store::references("environment", workspace, environment)
}

pub(crate) fn get(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<Option<String>> {
    get_in("environment", workspace, environment, variable)
}

pub(crate) fn get_auth(
    workspace: Uuid,
    environment: Uuid,
    variable: Uuid,
) -> Result<Option<String>> {
    get_in("auth", workspace, environment, variable)
}

fn get_in(
    domain: &str,
    workspace: Uuid,
    environment: Uuid,
    variable: Uuid,
) -> Result<Option<String>> {
    #[cfg(test)]
    {
        test_store::get(domain, workspace, environment, variable)
    }
    #[cfg(not(test))]
    match entry(domain, workspace, environment, variable)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(anyhow::anyhow!("无法从系统凭据存储读取密钥")),
    }
}

pub(crate) fn set(workspace: Uuid, environment: Uuid, variable: Uuid, value: &str) -> Result<()> {
    set_in("environment", workspace, environment, variable, value)
}

pub(crate) fn set_auth(
    workspace: Uuid,
    environment: Uuid,
    variable: Uuid,
    value: &str,
) -> Result<()> {
    set_in("auth", workspace, environment, variable, value)
}

fn set_in(
    domain: &str,
    workspace: Uuid,
    environment: Uuid,
    variable: Uuid,
    value: &str,
) -> Result<()> {
    #[cfg(test)]
    {
        test_store::set(domain, workspace, environment, variable, value)
    }
    #[cfg(not(test))]
    entry(domain, workspace, environment, variable)?
        .set_password(value)
        .map_err(|_| anyhow::anyhow!("无法安全保存密钥到系统凭据存储"))
}

pub(crate) fn delete(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<()> {
    delete_in("environment", workspace, environment, variable)
}

pub(crate) fn delete_auth(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<()> {
    delete_in("auth", workspace, environment, variable)
}

fn delete_in(domain: &str, workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<()> {
    #[cfg(test)]
    {
        test_store::delete(domain, workspace, environment, variable)
    }
    #[cfg(not(test))]
    match entry(domain, workspace, environment, variable)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(anyhow::anyhow!("无法从系统凭据存储删除密钥")),
    }
}

#[cfg(test)]
pub(crate) fn test_auth_references(workspace: Uuid, owner: Uuid) -> Result<Vec<Uuid>> {
    test_store::references("auth", workspace, owner)
}

#[cfg(test)]
mod test_store {
    use anyhow::Result;
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };
    use uuid::Uuid;

    type Credentials = HashMap<(String, Uuid, Uuid, Uuid), String>;

    static VALUES: OnceLock<Mutex<Credentials>> = OnceLock::new();

    fn values() -> &'static Mutex<Credentials> {
        VALUES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn get(
        domain: &str,
        workspace: Uuid,
        environment: Uuid,
        variable: Uuid,
    ) -> Result<Option<String>> {
        Ok(values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .get(&(domain.to_owned(), workspace, environment, variable))
            .cloned())
    }

    pub(super) fn set(
        domain: &str,
        workspace: Uuid,
        environment: Uuid,
        variable: Uuid,
        value: &str,
    ) -> Result<()> {
        values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .insert(
                (domain.to_owned(), workspace, environment, variable),
                value.to_owned(),
            );
        Ok(())
    }

    pub(super) fn delete(
        domain: &str,
        workspace: Uuid,
        environment: Uuid,
        variable: Uuid,
    ) -> Result<()> {
        values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .remove(&(domain.to_owned(), workspace, environment, variable));
        Ok(())
    }

    pub(super) fn references(
        domain: &str,
        workspace: Uuid,
        environment: Uuid,
    ) -> Result<Vec<Uuid>> {
        Ok(values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .keys()
            .filter_map(
                |(stored_domain, stored_workspace, stored_environment, reference)| {
                    (stored_domain == domain
                        && *stored_workspace == workspace
                        && *stored_environment == environment)
                        .then_some(*reference)
                },
            )
            .collect())
    }
}

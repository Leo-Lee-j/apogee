//! OS-backed storage for environment secrets. Keys are namespaced by workspace and
//! environment UUID so copied workspaces and same-named variables remain isolated.

use anyhow::Result;
use uuid::Uuid;

#[cfg(not(test))]
fn entry(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<keyring::Entry> {
    keyring::Entry::new(
        &format!("com.apogee.workspace.{workspace}.environment.{environment}"),
        &variable.to_string(),
    )
    .map_err(|_| anyhow::anyhow!("无法访问系统凭据存储"))
}

#[cfg(test)]
pub(crate) fn test_references(workspace: Uuid, environment: Uuid) -> Result<Vec<Uuid>> {
    test_store::references(workspace, environment)
}

pub(crate) fn get(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<Option<String>> {
    #[cfg(test)]
    {
        test_store::get(workspace, environment, variable)
    }
    #[cfg(not(test))]
    match entry(workspace, environment, variable)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(anyhow::anyhow!("无法从系统凭据存储读取环境密钥")),
    }
}

pub(crate) fn set(workspace: Uuid, environment: Uuid, variable: Uuid, value: &str) -> Result<()> {
    #[cfg(test)]
    {
        test_store::set(workspace, environment, variable, value)
    }
    #[cfg(not(test))]
    entry(workspace, environment, variable)?
        .set_password(value)
        .map_err(|_| anyhow::anyhow!("无法安全保存环境密钥到系统凭据存储"))
}

pub(crate) fn delete(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<()> {
    #[cfg(test)]
    {
        test_store::delete(workspace, environment, variable)
    }
    #[cfg(not(test))]
    match entry(workspace, environment, variable)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(anyhow::anyhow!("无法从系统凭据存储删除环境密钥")),
    }
}

#[cfg(test)]
mod test_store {
    use anyhow::Result;
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };
    use uuid::Uuid;

    type Credentials = HashMap<(Uuid, Uuid, Uuid), String>;

    static VALUES: OnceLock<Mutex<Credentials>> = OnceLock::new();

    fn values() -> &'static Mutex<Credentials> {
        VALUES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn get(
        workspace: Uuid,
        environment: Uuid,
        variable: Uuid,
    ) -> Result<Option<String>> {
        Ok(values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .get(&(workspace, environment, variable))
            .cloned())
    }

    pub(super) fn set(
        workspace: Uuid,
        environment: Uuid,
        variable: Uuid,
        value: &str,
    ) -> Result<()> {
        values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .insert((workspace, environment, variable), value.to_owned());
        Ok(())
    }

    pub(super) fn delete(workspace: Uuid, environment: Uuid, variable: Uuid) -> Result<()> {
        values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .remove(&(workspace, environment, variable));
        Ok(())
    }

    pub(super) fn references(workspace: Uuid, environment: Uuid) -> Result<Vec<Uuid>> {
        Ok(values()
            .lock()
            .map_err(|_| anyhow::anyhow!("credential test store poisoned"))?
            .keys()
            .filter_map(|(stored_workspace, stored_environment, reference)| {
                (*stored_workspace == workspace && *stored_environment == environment)
                    .then_some(*reference)
            })
            .collect())
    }
}

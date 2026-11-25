//! Export the launcher's CS3 security template for the isolated Squid smoke test.
use contracts::{ids::UserId, runtime::ContainerId};
use runner::launcher::{Error, Settings};

fn main() -> Result<(), Error> {
    let config = Settings::from_env()?.template(
        &ContainerId::Pr {
            pull_request_id: 7,
            payer: UserId("3".into()),
        },
        true,
        None,
    )?;
    println!(
        "{}",
        serde_json::to_string(&config).map_err(|_| Error::Protocol)?
    );
    Ok(())
}

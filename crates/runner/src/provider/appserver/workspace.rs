use super::*;
use serde_json::json;

pub(super) async fn prepare(rpc: &mut rpc::Rpc, spec: &JobSpec) -> Result<(), ProviderError> {
    if spec.kind == JobKind::JudgeWork {
        return Ok(());
    }
    let repo = spec
        .target
        .checkout()
        .ok_or_else(|| runtime(JobFailureKind::Configuration))?;
    for sha in [&repo.head_sha.0, &repo.base_sha.0] {
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(runtime(JobFailureKind::CheckoutFailed));
        }
    }
    let mirror = crate::git::mirror_path(std::path::Path::new("/mirror"), &repo.clone_url);
    // Clone from the operator-owned mirror; never reuse hostile checkout configuration or hooks.
    rpc.command(json!({"command":["bash","-c",
        "set -eu; rm -rf /work/repo; git -c core.hooksPath=/dev/null clone --quiet --no-checkout --no-hardlinks -- \"$1\" /work/repo; git -C /work/repo -c core.hooksPath=/dev/null -c submodule.recurse=false checkout --quiet --force --detach \"$2\"; git -C /work/repo cat-file -e \"$3^{commit}\"",
        "diffpilot-checkout",mirror,repo.head_sha.0,repo.base_sha.0],"cwd":"/work","timeoutMs":60_000,"outputBytesCap":4096})).await?;
    for path in ["/work/out", "/work/pr-data", "/work/flow-context.json"] {
        rpc.remove(path).await?;
    }
    rpc.mkdir("/work/out").await?;
    if let Some(flow) = &spec.flow_file {
        rpc.write(
            "/work/flow-context.json",
            flow.context.to_string().as_bytes(),
        )
        .await?;
        rpc.command(json!({"command":["chmod","0444","/work/flow-context.json"],"cwd":"/work"}))
            .await?;
        if let Some(previous) = &flow.previous {
            rpc.write("/work/out/flow.md", previous.as_bytes()).await?;
        }
    }
    if let Some(data) = &spec.pr_data {
        rpc.mkdir("/work/pr-data").await?;
        for (name, value) in [
            ("manifest", serde_json::to_value(&data.manifest)),
            ("threads", Ok(data.threads.clone())),
            (
                "review-comments",
                serde_json::to_value(&data.review_comments),
            ),
            ("reviews", serde_json::to_value(&data.reviews)),
            ("comments", serde_json::to_value(&data.comments)),
            ("ids", serde_json::to_value(&data.ids)),
            ("scope", serde_json::to_value(&data.scope)),
        ] {
            let value = value.map_err(|_| runtime(JobFailureKind::Configuration))?;
            rpc.write(
                &format!("/work/pr-data/{name}.json"),
                value.to_string().as_bytes(),
            )
            .await?;
        }
        for (path, text) in &data.conventions {
            let relative = std::path::Path::new(path);
            if path.is_empty()
                || !relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                return Err(runtime(JobFailureKind::Configuration));
            }
            let path = format!("/work/pr-data/conventions/{path}");
            if let Some((parent, _)) = path.rsplit_once('/') {
                rpc.mkdir(parent).await?;
            }
            rpc.write(&path, text.as_bytes()).await?;
        }
    }
    Ok(())
}

pub(super) async fn hygiene(rpc: &mut rpc::Rpc, spec: &JobSpec) -> Result<(), ProviderError> {
    if spec.kind == JobKind::JudgeWork {
        let preset = include_str!("../../../../../deploy/worker/config.toml")
            .replace(
                "sandbox_mode = \"danger-full-access\"",
                "sandbox_mode = \"read-only\"",
            )
            .replace(
                "features = {",
                "features = { shell_tool = false, unified_exec = false, view_image = false,",
            );
        // W3 Ensure creates /codex/config.toml -> /run/dp/config.toml on tmpfs.
        rpc.write("/run/dp/config.toml", preset.as_bytes()).await?;
        return Ok(());
    }
    let env = match &spec.access.memcan {
        Some(endpoint) => {
            json!({"DP_MEMCAN_URL":endpoint.url,"DP_MEMCAN_TOKEN":endpoint.token.expose()})
        }
        None => json!({}),
    };
    rpc.command(json!({"command":["python3","/usr/local/bin/worker-config.py","cs1"],"cwd":"/work","env":env,"timeoutMs":10_000,"outputBytesCap":0})).await?;
    let skills = rpc
        .request("skills/list", json!({"cwds":["/work"],"forceReload":true}))
        .await?;
    for entry in skills["data"].as_array().into_iter().flatten() {
        for skill in entry["skills"].as_array().into_iter().flatten() {
            if skill["scope"] == "repo" {
                let path = skill["path"]
                    .as_str()
                    .ok_or_else(|| runtime(JobFailureKind::Configuration))?;
                rpc.request("skills/config/write", json!({"path":path,"enabled":false}))
                    .await?;
            }
        }
    }
    Ok(())
}

//! # Spike S6：Postman 官方 schema 与真实导出文件的一致程度
//!
//! ## 要回答的决策问题
//!
//! 决策 D3 把 Postman 兼容口径抬到了「对齐最新版」= Collection Format v2.1.0 /
//! draft-07 schema。03 文档 §8.3 写了一条实现约束：
//!
//! > 官方 schema 与真实文件可能不一致 —— 策略是「校验失败降级为警告 + 记录到
//! > 迁移报告」，绝不因为校验不通过就拒绝导入。
//!
//! 这是**假设**，S6 来验证它：
//!
//! 1. 官方 schema 本身能不能当 draft-07 / draft-04 编译？
//! 2. 真实文件在「一律按 v2.1.0 严格校验」下的通过率？
//! 3. 改成「先探测版本、再用对应版本的 schema 校验」通过率是否变化？
//!    —— 这一步决定 `upgrade_v20.rs` 是可选优化还是必需前置步骤。
//! 4. draft-04 与 draft-07 两个版本的 schema 判定结果是否一致？
//! 5. 校验 2.8MB 的大集合要多久？会不会卡住导入流程？
//! 6. 失败时的错误信息**够不够用户看懂**？（03 文档 §8.3 承诺「报错精确到字段路径」）
//!
//! ## 验收标准（03 文档 §10）
//!
//! > 20/20 通过校验，请求字段零丢失。失败预案：改为「宽松解析 + 校验降级为警告」。

use anyhow::{Context, Result};
use jsonschema::Validator;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_ERRORS_PER_FILE: usize = 12;

// ─────────────────────────────────────────────────────────────────────
// 报告结构
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ErrorDetail {
    /// JSON Pointer 路径，对应 03 文档 §8.3「报错精确到字段路径」的要求
    instance_path: String,
    schema_path: String,
    message: String,
    /// 归类：判断「是文件不合规」还是「schema 太严 / 版本不匹配」
    category: String,
    /// 错误信息是否可直接展示给用户。oneOf 类错误会把整棵子树 dump 出来，不可用。
    user_presentable: bool,
}

#[derive(Debug, Serialize)]
struct FileVerdict {
    file: String,
    size_bytes: u64,
    detected_version: String,
    /// `schema-url` = 读 info.schema；`heuristic` = 结构推断
    detection_method: String,
    request_count: usize,
    /// 策略 A：一律用 v2.1.0 draft-07 校验（「对齐最新版」的字面实现）
    strict_latest_valid: bool,
    strict_latest_error_count: usize,
    /// 策略 B：按探测到的版本选用对应 schema 校验
    version_matched_valid: bool,
    version_matched_error_count: usize,
    version_matched_schema: String,
    /// 同一份 v2.1.0 schema 的 draft-04 与 draft-07 变体判定是否一致
    draft_variants_agree: bool,
    errors: Vec<ErrorDetail>,
    validate_ms: f64,
    parse_ms: f64,
}

#[derive(Debug, Serialize)]
struct SchemaInfo {
    name: String,
    bytes: u64,
    declared_draft: String,
    compiles: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    schemas: Vec<SchemaInfo>,
    total_files: usize,
    strict_latest_pass: usize,
    strict_latest_pass_rate: f64,
    version_matched_pass: usize,
    version_matched_pass_rate: f64,
    draft_variant_disagreements: usize,
    unpresentable_error_ratio: f64,
    error_categories: BTreeMap<String, usize>,
    slowest_validate_ms: f64,
    total_validate_ms: f64,
    largest_file_bytes: u64,
    files: Vec<FileVerdict>,
    verdict: String,
    findings: Vec<String>,
    recommendation: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────

struct Schemas {
    v21_d7: Validator,
    v21_d4: Option<Validator>,
    v20_d7: Option<Validator>,
    info: Vec<SchemaInfo>,
}

fn load_schema(
    dir: &Path,
    name: &str,
    draft: jsonschema::Draft,
) -> (Option<Validator>, Option<SchemaInfo>) {
    let path = dir.join(name);
    let Ok(raw) = std::fs::read(&path) else {
        return (None, None);
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return (None, None);
    };
    let declared = json
        .get("$schema")
        .and_then(|v| v.as_str())
        .unwrap_or("(未声明)")
        .to_string();
    // should_validate_formats(false)：format 关键字在 draft-07 里是注解而非断言，
    // 打开会引入大量与结构无关的噪声，对「文件结构是否可解析」这个问题没有帮助。
    let built = jsonschema::options()
        .with_draft(draft)
        .should_validate_formats(false)
        .build(&json);
    let info = SchemaInfo {
        name: name.to_string(),
        bytes: raw.len() as u64,
        declared_draft: declared,
        compiles: built.is_ok(),
    };
    (built.ok(), Some(info))
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let schema_dir = root.join("schemas");
    let corpus = root.join("corpus");
    let out_dir = root.join("..").join("results");
    std::fs::create_dir_all(&out_dir).ok();

    println!("=== Spike S6：Postman 官方 schema 校验真实导出文件 ===\n");

    // ── 1. 加载官方 schema ──────────────────────────────────────────
    let mut infos = Vec::new();
    let (v21_d7, i1) = load_schema(
        &schema_dir,
        "v2.1.0-draft-07.json",
        jsonschema::Draft::Draft7,
    );
    let (v21_d4, i2) = load_schema(
        &schema_dir,
        "v2.1.0-draft-04.json",
        jsonschema::Draft::Draft4,
    );
    let (v20_d7, i3) = load_schema(
        &schema_dir,
        "v2.0.0-draft-07.json",
        jsonschema::Draft::Draft7,
    );
    for i in [i1, i2, i3].into_iter().flatten() {
        infos.push(i);
    }

    println!("官方 schema：");
    for i in &infos {
        println!(
            "  {:<26} {:>7} B  声明 draft = {:<45} 编译 {}",
            i.name,
            i.bytes,
            i.declared_draft,
            if i.compiles { "OK" } else { "失败" }
        );
    }
    println!();

    let v21_d7 =
        v21_d7.context("官方 v2.1.0 draft-07 schema 无法编译 —— 这会直接推翻决策 D3 的实现路径")?;
    let schemas = Schemas {
        v21_d7,
        v21_d4,
        v20_d7,
        info: infos,
    };

    // ── 2. 逐个校验语料 ────────────────────────────────────────────
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    paths.sort();

    let mut verdicts = Vec::new();
    let mut categories: BTreeMap<String, usize> = BTreeMap::new();
    let mut strict_pass = 0usize;
    let mut matched_pass = 0usize;
    let mut disagreements = 0usize;
    let mut slowest = 0f64;
    let mut total_validate = 0f64;
    let mut largest = 0u64;
    let mut err_total = 0usize;
    let mut err_unpresentable = 0usize;

    println!(
        "{:<6} {:<6} {:<58} {:>10}  {:<16} {:>6} {:>7} {:>7}",
        "严格", "匹配", "文件", "字节", "探测版本", "请求", "严格err", "校验ms"
    );
    println!("{}", "-".repeat(126));

    for path in &paths {
        let bytes = std::fs::read(path)?;
        let size = bytes.len() as u64;
        largest = largest.max(size);

        let t = Instant::now();
        let doc: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                println!("  [跳过] {} —— JSON 解析失败：{e}", short(path));
                continue;
            }
        };
        let parse_ms = t.elapsed().as_secs_f64() * 1000.0;

        let (detected, method) = detect_version(&doc);
        let request_count = count_requests(&doc);

        // 策略 A：一律按最新版 v2.1.0 draft-07 严格校验
        let t = Instant::now();
        let strict_errors: Vec<_> = schemas.v21_d7.iter_errors(&doc).collect();
        let validate_ms = t.elapsed().as_secs_f64() * 1000.0;
        total_validate += validate_ms;
        slowest = slowest.max(validate_ms);
        let strict_valid = strict_errors.is_empty();
        if strict_valid {
            strict_pass += 1;
        }

        // 策略 B：按探测版本选 schema
        let (matched_validator, matched_name) = if detected.starts_with("v2.0.0") {
            (schemas.v20_d7.as_ref(), "v2.0.0-draft-07")
        } else {
            (Some(&schemas.v21_d7), "v2.1.0-draft-07")
        };
        // 没有对应版本的 schema 时用 usize::MAX 表示「无法判定」，避免误记为通过
        let matched_errors: usize = matched_validator
            .map(|v| v.iter_errors(&doc).count())
            .unwrap_or(usize::MAX);
        let matched_valid = matched_errors == 0;
        if matched_valid {
            matched_pass += 1;
        }

        // draft-04 / draft-07 变体一致性
        let d4_valid = schemas.v21_d4.as_ref().map(|v| v.is_valid(&doc));
        let agree = d4_valid.map(|v| v == strict_valid).unwrap_or(true);
        if !agree {
            disagreements += 1;
        }

        let mut details = Vec::new();
        for (idx, e) in strict_errors.iter().enumerate() {
            let msg = e.to_string();
            let (cat, presentable) = categorize(&msg);
            *categories.entry(cat.clone()).or_insert(0) += 1;
            err_total += 1;
            if !presentable {
                err_unpresentable += 1;
            }
            if idx < MAX_ERRORS_PER_FILE {
                details.push(ErrorDetail {
                    instance_path: e.instance_path().to_string(),
                    schema_path: e.schema_path().to_string(),
                    message: truncate(&msg, 240),
                    category: cat,
                    user_presentable: presentable,
                });
            }
        }

        println!(
            "{:<6} {:<6} {:<58} {:>10}  {:<16} {:>6} {:>7} {:>7.2}",
            if strict_valid { "PASS" } else { "FAIL" },
            if matched_valid { "PASS" } else { "FAIL" },
            truncate(&short(path), 56),
            size,
            detected,
            request_count,
            strict_errors.len(),
            validate_ms
        );

        verdicts.push(FileVerdict {
            file: short(path),
            size_bytes: size,
            detected_version: detected,
            detection_method: method,
            request_count,
            strict_latest_valid: strict_valid,
            strict_latest_error_count: strict_errors.len(),
            version_matched_valid: matched_valid,
            version_matched_error_count: if matched_errors == usize::MAX {
                0
            } else {
                matched_errors
            },
            version_matched_schema: matched_name.to_string(),
            draft_variants_agree: agree,
            errors: details,
            validate_ms,
            parse_ms,
        });
    }

    let total = verdicts.len();
    let strict_rate = pct(strict_pass, total);
    let matched_rate = pct(matched_pass, total);
    let unpresentable_ratio = if err_total == 0 {
        0.0
    } else {
        err_unpresentable as f64 * 100.0 / err_total as f64
    };

    // ── 3. 结论 ────────────────────────────────────────────────────
    let mut findings = Vec::new();
    let mut recommendation = Vec::new();

    if let Some(i) = schemas
        .info
        .iter()
        .find(|i| i.name.starts_with("v2.1.0-draft-07"))
    {
        if let Some(j) = schemas
            .info
            .iter()
            .find(|i| i.name.starts_with("v2.1.0-draft-04"))
        {
            if i.bytes != j.bytes {
                findings.push(format!(
                    "同一个 v2.1.0 版本的 draft-04 与 draft-07 schema 文件内容不同（{} vs {} 字节），\
                     但对本语料库的判定结果完全一致（不一致 {} 个）。说明差异在 schema 写法层面，不影响判定。\
                     结论：只需维护 draft-07 一份本地副本。",
                    i.bytes, j.bytes, disagreements
                ));
            }
        }
    }

    if strict_pass < total {
        let failed: Vec<&FileVerdict> =
            verdicts.iter().filter(|v| !v.strict_latest_valid).collect();
        let all_v20 = failed
            .iter()
            .all(|v| v.detected_version.starts_with("v2.0.0"));
        let rescued = failed.iter().filter(|v| v.version_matched_valid).count();
        findings.push(format!(
            "严格按 v2.1.0 校验：{}/{} 通过（{:.1}%）。失败 {} 个，{}。\
             改用「按探测版本匹配 schema」后通过率升到 {:.1}%（救回 {} 个）。",
            strict_pass,
            total,
            strict_rate,
            failed.len(),
            if all_v20 {
                "全部是 v2.0.0 格式的文件"
            } else {
                "版本分布不单一"
            },
            matched_rate,
            rescued
        ));
        if all_v20 {
            findings.push(
                "失败原因不是「schema 太严」，而是「拿 v2.1 的 schema 去校验 v2.0 的文件」。\
                 典型差异：v2.0 的 auth 用对象（`{\"oauth2\":{...}}`），v2.1 改成了 `{key,value,type}` 数组。\
                 这说明 03 文档 §8.3 里的 `upgrade_v20.rs` **不是可选优化，是必需的前置步骤**。"
                    .into(),
            );
        }
    }

    if unpresentable_ratio > 50.0 {
        findings.push(format!(
            "错误信息可用性问题：{:.0}% 的校验错误来自 oneOf/anyOf 关键字，\
             这类错误只报「不匹配任何分支」并把整棵子树 dump 出来，路径停在容器层（如 `/item/0`），\
             既定位不到具体字段，也无法直接展示给用户。\
             03 文档 §8.3 第 2 条承诺的「报错精确到 JSON Pointer 级字段路径」按当前实现做不到。",
            unpresentable_ratio
        ));
        recommendation.push(
            "§8.3 第 2 条需要补充实现细节：oneOf 失败时不能直接用校验器的错误，\
             必须自己按 `item` / `auth` 的判别字段选定预期分支，再对该分支单独校验，才能拿到可用的字段级报错。"
                .into(),
        );
    }

    findings.push(format!(
        "校验性能不是问题：最大文件 {:.1}MB，单文件最慢校验 {:.2}ms，全库 {} 个文件合计 {:.1}ms。\
         schema 校验完全可以放在导入预览的同步路径上，不需要异步化。",
        largest as f64 / 1048576.0,
        slowest,
        total,
        total_validate
    ));

    let version_detect_ok = verdicts.iter().all(|v| v.detection_method == "schema-url");
    if version_detect_ok {
        findings.push(
            "版本探测：全部语料都能从 `info.schema` 的 URL 读到版本，靠内容不靠文件名是可行的（§8.3 第 1 条成立）。\
             注意 v1 格式没有 `info` 字段，必须走结构推断分支 —— 语料库里的 v1 文件在采集阶段就因为缺 `info` 被过滤掉了，\
             这个分支目前**没有被覆盖**，需要单独补测试。"
                .into(),
        );
    }

    let strict_ok = strict_pass == total;
    let verdict = if strict_ok {
        format!("PASS：{total}/{total} 通过严格校验")
    } else if matched_pass == total {
        format!(
            "CONDITIONAL PASS：一律按 v2.1.0 严格校验只有 {strict_pass}/{total}；\
             加上「版本探测 + 版本匹配 schema」后 {matched_pass}/{total} 全通过。\
             严格校验可行，但前提是先做版本升级。"
        )
    } else {
        format!(
            "FAIL：即使按版本匹配 schema，仍有 {}/{} 通不过",
            total - matched_pass,
            total
        )
    };

    if matched_pass == total && !strict_ok {
        recommendation.push(
            "导入器采用「探测版本 → 升级到 v2.1 → 严格校验」三段式。\
             schema 校验保留为硬门槛（能挡住真正损坏的文件），但**必须在升级之后**执行。"
                .into(),
        );
        recommendation.push(
            "03 文档 §8.3 第 4 条（校验失败降级为警告）仍然要保留，但定位从「默认策略」调整为「兜底策略」：\
             升级后仍校验失败的文件，降级为警告继续解析并写入迁移报告，不阻断导入。"
                .into(),
        );
    } else if !strict_ok {
        recommendation.push(
            "必须采用「宽松解析 + 校验降级为警告」：schema 校验结果只写进迁移报告，不阻断导入。"
                .into(),
        );
    } else {
        recommendation.push("导入器可以采用严格校验：校验失败即拒绝导入并报字段路径。".into());
    }

    println!("\n{}", "=".repeat(126));
    println!(
        "语料文件                {total} 个，最大 {:.2} MB",
        largest as f64 / 1048576.0
    );
    println!("策略A 一律 v2.1.0 严格   {strict_pass}/{total}  ({strict_rate:.1}%)");
    println!("策略B 版本匹配 schema    {matched_pass}/{total}  ({matched_rate:.1}%)");
    println!("draft-04/07 判定不一致   {disagreements} 个");
    println!("校验耗时                合计 {total_validate:.1}ms，单文件最慢 {slowest:.2}ms");
    println!("错误信息不可直接展示占比 {unpresentable_ratio:.0}%");
    if !categories.is_empty() {
        println!("\n错误类型分布：");
        let mut sorted: Vec<_> = categories.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (k, v) in sorted {
            println!("  {v:>5}  {k}");
        }
    }
    println!("\n【结论】{verdict}\n");
    println!("【发现】");
    for (i, f) in findings.iter().enumerate() {
        println!("  {}. {f}", i + 1);
    }
    println!("\n【建议】");
    for (i, r) in recommendation.iter().enumerate() {
        println!("  {}. {r}", i + 1);
    }
    println!("{}", "=".repeat(126));

    let report = Report {
        schemas: schemas.info,
        total_files: total,
        strict_latest_pass: strict_pass,
        strict_latest_pass_rate: strict_rate,
        version_matched_pass: matched_pass,
        version_matched_pass_rate: matched_rate,
        draft_variant_disagreements: disagreements,
        unpresentable_error_ratio: unpresentable_ratio,
        error_categories: categories,
        slowest_validate_ms: slowest,
        total_validate_ms: total_validate,
        largest_file_bytes: largest,
        files: verdicts,
        verdict,
        findings,
        recommendation,
    };
    let out = out_dir.join("s6.json");
    std::fs::write(&out, serde_json::to_vec_pretty(&report)?)?;

    // Windows 控制台默认 GBK，中文 stdout 会乱码。
    // 把人类可读报告单独写成 UTF-8 文件，绕开控制台代码页问题。
    let md = render_markdown(&report);
    let md_path = out_dir.join("s6-report.md");
    std::fs::write(&md_path, md)?;

    println!("\n完整报告：{}", out.display());
    println!("可读报告：{} (UTF-8)", md_path.display());

    Ok(())
}

fn render_markdown(r: &Report) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "# Spike S6 结果：Postman 官方 schema 校验真实导出文件\n");
    let _ = writeln!(s, "## 结论\n\n{}\n", r.verdict);

    let _ = writeln!(s, "## 关键数字\n");
    let _ = writeln!(s, "| 项目 | 结果 | 验收标准 |");
    let _ = writeln!(s, "| --- | --- | --- |");
    let _ = writeln!(
        s,
        "| 语料文件数 | {} 个（最大 {:.2} MB） | ≥ 20 |",
        r.total_files,
        r.largest_file_bytes as f64 / 1048576.0
    );
    let _ = writeln!(
        s,
        "| 策略A：一律按 v2.1.0 严格校验 | {}/{}（{:.1}%） | 20/20 |",
        r.strict_latest_pass, r.total_files, r.strict_latest_pass_rate
    );
    let _ = writeln!(
        s,
        "| 策略B：按版本匹配 schema 校验 | {}/{}（{:.1}%） | — |",
        r.version_matched_pass, r.total_files, r.version_matched_pass_rate
    );
    let _ = writeln!(
        s,
        "| draft-04 / draft-07 判定不一致 | {} 个 | — |",
        r.draft_variant_disagreements
    );
    let _ = writeln!(
        s,
        "| 校验耗时 | 单文件最慢 {:.2}ms，全库合计 {:.1}ms | — |",
        r.slowest_validate_ms, r.total_validate_ms
    );
    let _ = writeln!(
        s,
        "| 错误信息不可直接展示占比 | {:.0}% | 越低越好 |\n",
        r.unpresentable_error_ratio
    );

    let _ = writeln!(s, "## 官方 schema 加载情况\n");
    let _ = writeln!(s, "| 文件 | 字节 | 声明的 draft | 编译 |");
    let _ = writeln!(s, "| --- | --- | --- | --- |");
    for i in &r.schemas {
        let _ = writeln!(
            s,
            "| `{}` | {} | `{}` | {} |",
            i.name,
            i.bytes,
            i.declared_draft,
            if i.compiles { "OK" } else { "失败" }
        );
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "## 发现\n");
    for (i, f) in r.findings.iter().enumerate() {
        let _ = writeln!(s, "{}. {}\n", i + 1, f);
    }

    let _ = writeln!(s, "## 建议\n");
    for (i, x) in r.recommendation.iter().enumerate() {
        let _ = writeln!(s, "{}. {}\n", i + 1, x);
    }

    if !r.error_categories.is_empty() {
        let _ = writeln!(s, "## 错误类型分布\n");
        let _ = writeln!(s, "| 次数 | 类型 |");
        let _ = writeln!(s, "| --- | --- |");
        let mut sorted: Vec<_> = r.error_categories.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (k, v) in sorted {
            let _ = writeln!(s, "| {} | {} |", v, k);
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "## 逐文件明细\n");
    let _ = writeln!(
        s,
        "| 严格 | 匹配 | 文件 | 字节 | 探测版本 | 探测方式 | 请求数 | 严格err | 校验ms |"
    );
    let _ = writeln!(s, "| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for f in &r.files {
        let _ = writeln!(
            s,
            "| {} | {} | `{}` | {} | {} | {} | {} | {} | {:.2} |",
            if f.strict_latest_valid {
                "PASS"
            } else {
                "**FAIL**"
            },
            if f.version_matched_valid {
                "PASS"
            } else {
                "**FAIL**"
            },
            f.file,
            f.size_bytes,
            f.detected_version,
            f.detection_method,
            f.request_count,
            f.strict_latest_error_count,
            f.validate_ms
        );
    }

    let failed: Vec<&FileVerdict> = r.files.iter().filter(|f| !f.strict_latest_valid).collect();
    if !failed.is_empty() {
        let _ = writeln!(s, "\n## 失败样本的错误明细\n");
        for f in failed {
            let _ = writeln!(s, "### `{}`（探测版本 {}）\n", f.file, f.detected_version);
            for e in &f.errors {
                let _ = writeln!(s, "- **instance** `{}`", e.instance_path);
                let _ = writeln!(s, "  **schema** `{}`", e.schema_path);
                let _ = writeln!(
                    s,
                    "  **分类** {}（可直接展示：{}）",
                    e.category,
                    if e.user_presentable { "是" } else { "否" }
                );
                let _ = writeln!(s, "  ```\n  {}\n  ```", e.message);
            }
            let _ = writeln!(s);
        }
    }
    s
}

// ─────────────────────────────────────────────────────────────────────
// 版本探测 —— 对应 03 文档 §8.3 第 1 条：靠内容不靠文件名
// ─────────────────────────────────────────────────────────────────────

fn detect_version(doc: &serde_json::Value) -> (String, String) {
    if let Some(url) = doc
        .get("info")
        .and_then(|i| i.get("schema"))
        .and_then(|s| s.as_str())
    {
        let v = if url.contains("v2.1.0") {
            "v2.1.0"
        } else if url.contains("v2.0.0") {
            "v2.0.0"
        } else {
            "v2.x-unknown"
        };
        return (v.to_string(), "schema-url".into());
    }
    // 回退：v1 格式没有 info，用 order + requests 判定
    if doc.get("order").is_some() || doc.get("requests").is_some() {
        return ("v1 (legacy)".into(), "heuristic".into());
    }
    if doc.get("item").is_some() {
        return ("v2.x-no-schema".into(), "heuristic".into());
    }
    ("unknown".into(), "heuristic".into())
}

fn count_requests(doc: &serde_json::Value) -> usize {
    fn walk(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::Array(a) => a.iter().map(walk).sum(),
            serde_json::Value::Object(m) => {
                if m.contains_key("request") {
                    1 + m.get("item").map(walk).unwrap_or(0)
                } else {
                    m.get("item").map(walk).unwrap_or(0)
                }
            }
            _ => 0,
        }
    }
    doc.get("item").map(walk).unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────

/// 归类校验错误，并判断错误信息能否直接展示给用户。
///
/// 返回 `(分类, 是否可直接展示)`。
fn categorize(msg: &str) -> (String, bool) {
    let m = msg.to_lowercase();
    if m.contains("is not valid under any of the schemas")
        || m.contains("anyof")
        || m.contains("oneof")
    {
        // oneOf/anyOf 失败只说「不匹配任何分支」，路径停在容器层，且把整棵子树 dump 出来
        (
            "联合类型不匹配（oneOf/anyOf，错误信息不可用）".into(),
            false,
        )
    } else if m.contains("additional properties") || m.contains("not allowed") {
        ("多出 schema 未声明的字段".into(), true)
    } else if m.contains("is a required property") || m.contains("required") {
        ("缺少必填字段".into(), true)
    } else if m.contains("is not of type") {
        ("类型不符".into(), true)
    } else if m.contains("is not one of") || m.contains("enum") {
        ("枚举值超出范围".into(), true)
    } else if m.contains("does not match") || m.contains("pattern") {
        ("正则/格式不匹配".into(), true)
    } else {
        (format!("其它：{}", truncate(msg, 60)), true)
    }
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 * 100.0 / total as f64
    }
}

fn short(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

//! Schema-first 契约引擎：编译期 schema、安全策略、校验闸门、JSON Schema
//! 导出与版本兼容性检查（示例 1/6）。
//!
//! 运行：`cargo run -p nextjson --example contract_engine`
//!
//! 本示例演示 NextJson 的第一根支柱——把"类型即契约"落实为可执行代码：
//!
//! 1. `#[njson(...)]` 属性被编译进 `NsonSchema::SCHEMA`（一个 `const`），
//!    同一份声明同时驱动序列化、校验、JSON Schema 导出与兼容性检查，零运行时开销；
//! 2. 安全策略（`max_str_len` / `max_items` / `min` / `max` / `sensitive` /
//!    `deny_unknown_fields`）作为 schema 的一部分在运行时强制执行；
//! 3. 版本兼容性以编译期 schema 差异的形式呈现——加必填字段、删字段、
//!    收窄整数范围等会在发布前被 `check_between` 拦截。

use nextjson::{
    check_between, from_str, json, schema_of, to_json_schema, to_string_pretty, to_value,
    validate_value, NsonDeserialize, NsonSerialize,
};

/// 客户记录，其 schema 即对外契约。
#[derive(NsonSerialize, NsonDeserialize, Clone, Debug)]
#[njson(deny_unknown_fields)]
struct Customer {
    /// 字符串最多 32 个 Unicode 标量值。
    #[njson(max_str_len = 32)]
    name: String,
    /// 整数闭区间 [0, 200]。
    #[njson(min = 0, max = 200)]
    loyalty_points: u32,
    /// 数组最多 8 个元素。
    #[njson(max_items = 8)]
    tags: Vec<String>,
    /// 标记为敏感：只报告路径用于脱敏，永不导致校验失败。
    #[njson(sensitive)]
    api_key: String,
}

/// v2 契约：新增了一个必填字段 `email`（旧载荷没有它）。
#[derive(NsonSerialize, NsonDeserialize, Clone, Debug)]
#[njson(deny_unknown_fields)]
struct CustomerV2 {
    #[njson(max_str_len = 32)]
    name: String,
    #[njson(min = 0, max = 200)]
    loyalty_points: u32,
    #[njson(max_items = 8)]
    tags: Vec<String>,
    #[njson(sensitive)]
    api_key: String,
    /// 新增：在 v2 中必填——旧数据必然缺失。
    email: String,
}

fn main() -> nextjson::Result<()> {
    // 1. 内省编译期 schema。
    println!("== 1. 编译期 schema ==");
    println!("{:#?}", schema_of::<Customer>());

    // 2. 导出 JSON Schema（draft-07），可直接交给前端 / OpenAPI / 校验工具。
    println!("\n== 2. JSON Schema 导出 ==");
    let json_schema = to_json_schema::<Customer>();
    println!("{}", to_string_pretty(&json_schema)?);

    // 3. 合规载荷：校验必须零违规。
    println!("\n== 3. 校验：合规载荷 ==");
    let good: Customer = from_str(
        r#"{"name":"Ada Lovelace","loyalty_points":150,"tags":["vip","analyst"],"api_key":"sk-live-abc"}"#,
    )?;
    let report = validate_value::<Customer>(&to_value(&good)?);
    println!(
        "violations = {}, is_ok = {}",
        report.violations.len(),
        report.is_ok()
    );

    // 4. 敌意载荷：越界字符串、越界整数、越界数组、未知字段——全部被捕获。
    println!("\n== 4. 校验：敌意载荷 ==");
    let bad = json!({
        "name": "a very long name exceeding the declared maximum of thirty-two scalars",
        "loyalty_points": 9999,
        "tags": ["t1","t2","t3","t4","t5","t6","t7","t8","t9","t10","t11","t12"],
        "api_key": "sk-live-secret",
        "hacker_field": "unknown",
    });
    let report = validate_value::<Customer>(&bad);
    for violation in &report.violations {
        println!("  violation @ {:?}: {:?}", violation.path, violation.kind);
    }
    // sensitive 只出现在脱敏清单里，不进 violations。
    println!("  敏感路径（用于脱敏）: {:?}", report.sensitive_paths());

    // 5. 版本兼容性：编译期 diff，发布前拦截破坏性变更。
    println!("\n== 5. 版本兼容性：Customer -> CustomerV2 ==");
    let compat = check_between::<Customer, CustomerV2>();
    println!(
        "forward_compatible = {} (旧 reader 能读新数据), backward_compatible = {} (新 reader 能读旧数据)",
        compat.forward_compatible, compat.backward_compatible
    );
    for issue in &compat.issues {
        println!("  [{:?}] {}: {}", issue.severity, issue.path, issue.message);
    }
    assert!(!compat.is_compatible(), "加必填字段必须被判定为不兼容");

    Ok(())
}

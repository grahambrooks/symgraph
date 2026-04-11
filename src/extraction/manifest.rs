//! Package manager manifest file extraction
//!
//! Extracts dependency information from common package manager files:
//! - package.json (npm/yarn/pnpm)
//! - Cargo.toml (Rust)
//! - go.mod (Go)
//! - pyproject.toml (Python)
//! - requirements.txt (Python pip)
//! - Gemfile (Ruby Bundler)
//! - composer.json (PHP Composer)
//! - pom.xml (Maven)
//! - build.gradle / build.gradle.kts (Gradle)
//! - build.sbt (SBT / Scala)

use std::path::Path;

use crate::types::{Edge, EdgeKind, ExtractionResult, Language, Node, NodeKind, Visibility};

/// Known manifest filenames
const MANIFEST_FILES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "build.sbt",
];

/// Check if a filename is a recognized package manager manifest
pub fn is_manifest_file(filename: &str) -> bool {
    MANIFEST_FILES.contains(&filename)
}

/// Get the ecosystem language for a manifest file
pub fn manifest_language(filename: &str) -> Language {
    match filename {
        "package.json" => Language::JavaScript,
        "Cargo.toml" => Language::Rust,
        "go.mod" => Language::Go,
        "pyproject.toml" | "requirements.txt" => Language::Python,
        "Gemfile" => Language::Ruby,
        "composer.json" => Language::Php,
        "pom.xml" => Language::Java,
        "build.gradle" => Language::Groovy,
        "build.gradle.kts" => Language::Kotlin,
        "build.sbt" => Language::Scala,
        _ => Language::Unknown,
    }
}

/// Extract symbols from a package manager manifest file
pub fn extract_manifest<P: AsRef<Path>>(path: P, content: &str) -> ExtractionResult {
    let path = path.as_ref();
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let language = manifest_language(filename);
    let file_path = path.display().to_string();

    let mut result = ExtractionResult::default();
    let mut next_id: i64 = 1;

    // Create file node
    let file_node = Node {
        id: next_id,
        kind: NodeKind::File,
        name: filename.to_string(),
        qualified_name: Some(file_path.clone()),
        file_path: file_path.clone(),
        start_line: 0,
        end_line: content.lines().count() as u32,
        start_column: 0,
        end_column: 0,
        signature: None,
        visibility: Visibility::Public,
        docstring: None,
        is_async: false,
        is_static: false,
        is_exported: true,
        language,
    };
    let file_id = next_id;
    next_id += 1;
    result.nodes.push(file_node);

    match filename {
        "package.json" => extract_package_json(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "Cargo.toml" => extract_cargo_toml(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "go.mod" => extract_go_mod(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "pyproject.toml" => extract_pyproject_toml(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "requirements.txt" => extract_requirements_txt(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "Gemfile" => extract_gemfile(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "composer.json" => extract_composer_json(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "pom.xml" => extract_pom_xml(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "build.gradle" | "build.gradle.kts" => extract_gradle(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        "build.sbt" => extract_build_sbt(
            content,
            &file_path,
            language,
            file_id,
            &mut next_id,
            &mut result,
        ),
        _ => {}
    }

    result
}

#[allow(clippy::too_many_arguments)]
fn add_node(
    result: &mut ExtractionResult,
    next_id: &mut i64,
    parent_id: i64,
    kind: NodeKind,
    name: String,
    file_path: &str,
    language: Language,
    line: u32,
    signature: Option<String>,
) -> i64 {
    let id = *next_id;
    *next_id += 1;

    result.nodes.push(Node {
        id,
        kind,
        name,
        qualified_name: None,
        file_path: file_path.to_string(),
        start_line: line,
        end_line: line,
        start_column: 0,
        end_column: 0,
        signature,
        visibility: Visibility::Public,
        docstring: None,
        is_async: false,
        is_static: false,
        is_exported: true,
        language,
    });

    result.edges.push(Edge {
        id: 0,
        source_id: parent_id,
        target_id: id,
        kind: EdgeKind::Contains,
        file_path: Some(file_path.to_string()),
        line: Some(line),
        column: Some(0),
    });

    id
}

// ── package.json ──

fn extract_package_json(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(content) else {
        return;
    };

    // Package name
    if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
        let version = json.get("version").and_then(|v| v.as_str());
        let sig = version.map(|v| format!("{name}@{v}"));
        add_node(
            result,
            next_id,
            file_id,
            NodeKind::Module,
            name.to_string(),
            file_path,
            language,
            1,
            sig,
        );
    }

    // Dependencies
    for dep_key in &[
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = json.get(*dep_key).and_then(|v| v.as_object()) {
            for (name, version) in deps {
                let sig = version.as_str().map(|v| format!("{dep_key}: {v}"));
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name.clone(),
                    file_path,
                    language,
                    1,
                    sig,
                );
            }
        }
    }

    // Scripts
    if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
        for (name, cmd) in scripts {
            let sig = cmd.as_str().map(|c| c.to_string());
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Function,
                name.clone(),
                file_path,
                language,
                1,
                sig,
            );
        }
    }
}

// ── Cargo.toml ──

fn extract_cargo_toml(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    let Ok(toml) = content.parse::<toml::Value>() else {
        return;
    };

    // Package name
    if let Some(name) = toml
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    {
        let version = toml
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str());
        let sig = version.map(|v| format!("{name}@{v}"));
        add_node(
            result,
            next_id,
            file_id,
            NodeKind::Module,
            name.to_string(),
            file_path,
            language,
            1,
            sig,
        );
    }

    // Dependencies, dev-dependencies, build-dependencies
    for dep_key in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = toml.get(*dep_key).and_then(|v| v.as_table()) {
            for (name, spec) in deps {
                let sig = match spec {
                    toml::Value::String(v) => Some(format!("{dep_key}: {v}")),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(|v| format!("{dep_key}: {v}")),
                    _ => None,
                };
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name.clone(),
                    file_path,
                    language,
                    1,
                    sig,
                );
            }
        }
    }

    // Features
    if let Some(features) = toml.get("features").and_then(|v| v.as_table()) {
        for (name, _) in features {
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Constant,
                name.clone(),
                file_path,
                language,
                1,
                Some("feature".to_string()),
            );
        }
    }
}

// ── go.mod ──

fn extract_go_mod(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    let mut in_require = false;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Module declaration
        if let Some(module) = trimmed.strip_prefix("module ") {
            let module = module.trim();
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Module,
                module.to_string(),
                file_path,
                language,
                line_num as u32 + 1,
                None,
            );
            continue;
        }

        // Go version
        if let Some(version) = trimmed.strip_prefix("go ") {
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Constant,
                "go".to_string(),
                file_path,
                language,
                line_num as u32 + 1,
                Some(version.trim().to_string()),
            );
            continue;
        }

        // Require block
        if trimmed == "require (" {
            in_require = true;
            continue;
        }
        if trimmed == ")" {
            in_require = false;
            continue;
        }

        // Single-line require
        if let Some(req) = trimmed.strip_prefix("require ") {
            let req = req.trim();
            if let Some((path, version)) = req.split_once(' ') {
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    path.trim().to_string(),
                    file_path,
                    language,
                    line_num as u32 + 1,
                    Some(version.trim().to_string()),
                );
            }
            continue;
        }

        // Inside require block
        if in_require && !trimmed.is_empty() && !trimmed.starts_with("//") {
            let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
            if let Some(&path) = parts.first() {
                let version = parts.get(1).map(|v| v.trim().to_string());
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    path.to_string(),
                    file_path,
                    language,
                    line_num as u32 + 1,
                    version,
                );
            }
        }
    }
}

// ── pyproject.toml ──

fn extract_pyproject_toml(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    let Ok(toml) = content.parse::<toml::Value>() else {
        return;
    };

    // Project name (PEP 621)
    if let Some(name) = toml
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    {
        let version = toml
            .get("project")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str());
        let sig = version.map(|v| format!("{name}@{v}"));
        add_node(
            result,
            next_id,
            file_id,
            NodeKind::Module,
            name.to_string(),
            file_path,
            language,
            1,
            sig,
        );
    }

    // Poetry project name
    if let Some(name) = toml
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    {
        if !result
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Module && n.name != filename_from_path(file_path))
        {
            let version = toml
                .get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("version"))
                .and_then(|v| v.as_str());
            let sig = version.map(|v| format!("{name}@{v}"));
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Module,
                name.to_string(),
                file_path,
                language,
                1,
                sig,
            );
        }
    }

    // PEP 621 dependencies
    if let Some(deps) = toml
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        for dep in deps {
            if let Some(dep_str) = dep.as_str() {
                let (name, version) = parse_pep508_dependency(dep_str);
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name,
                    file_path,
                    language,
                    1,
                    version,
                );
            }
        }
    }

    // PEP 621 optional-dependencies
    if let Some(groups) = toml
        .get("project")
        .and_then(|p| p.get("optional-dependencies"))
        .and_then(|d| d.as_table())
    {
        for (_group, deps) in groups {
            if let Some(deps) = deps.as_array() {
                for dep in deps {
                    if let Some(dep_str) = dep.as_str() {
                        let (name, version) = parse_pep508_dependency(dep_str);
                        add_node(
                            result,
                            next_id,
                            file_id,
                            NodeKind::Import,
                            name,
                            file_path,
                            language,
                            1,
                            version,
                        );
                    }
                }
            }
        }
    }

    // Poetry dependencies
    for dep_key in &["dependencies", "dev-dependencies"] {
        if let Some(deps) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get(*dep_key))
            .and_then(|d| d.as_table())
        {
            for (name, spec) in deps {
                if name == "python" {
                    continue;
                }
                let sig = match spec {
                    toml::Value::String(v) => Some(v.clone()),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string()),
                    _ => None,
                };
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name.clone(),
                    file_path,
                    language,
                    1,
                    sig,
                );
            }
        }
    }

    // uv dependencies (tool.uv.dev-dependencies)
    if let Some(deps) = toml
        .get("tool")
        .and_then(|t| t.get("uv"))
        .and_then(|u| u.get("dev-dependencies"))
        .and_then(|d| d.as_array())
    {
        for dep in deps {
            if let Some(dep_str) = dep.as_str() {
                let (name, version) = parse_pep508_dependency(dep_str);
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name,
                    file_path,
                    language,
                    1,
                    version,
                );
            }
        }
    }
}

fn filename_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Parse a PEP 508 dependency string like "requests>=2.25.0" into (name, version_spec)
fn parse_pep508_dependency(dep: &str) -> (String, Option<String>) {
    // Split on first version specifier character
    let name_end = dep.find(['>', '<', '=', '!', '~', '[', ';', ' ']);
    match name_end {
        Some(pos) => {
            let name = dep[..pos].trim().to_string();
            let rest = dep[pos..].trim();
            // Strip extras like [dev] and environment markers
            let version = if rest.starts_with('[') {
                rest.find(']')
                    .map(|end| rest[end + 1..].trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                // Strip environment markers (after ;)
                let version_part = rest.split(';').next().unwrap_or(rest).trim();
                if version_part.is_empty() {
                    None
                } else {
                    Some(version_part.to_string())
                }
            };
            (name, version)
        }
        None => (dep.trim().to_string(), None),
    }
}

// ── requirements.txt ──

fn extract_requirements_txt(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines, comments, and options
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }

        let (name, version) = parse_pep508_dependency(trimmed);
        if !name.is_empty() {
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Import,
                name,
                file_path,
                language,
                line_num as u32 + 1,
                version,
            );
        }
    }
}

// ── Gemfile ──

fn extract_gemfile(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // source declaration
        if let Some(source) = trimmed.strip_prefix("source ") {
            let source = source.trim().trim_matches(|c| c == '\'' || c == '"');
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Constant,
                "source".to_string(),
                file_path,
                language,
                line_num as u32 + 1,
                Some(source.to_string()),
            );
            continue;
        }

        // ruby version
        if let Some(version) = trimmed.strip_prefix("ruby ") {
            let version = version.trim().trim_matches(|c| c == '\'' || c == '"');
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Constant,
                "ruby".to_string(),
                file_path,
                language,
                line_num as u32 + 1,
                Some(version.to_string()),
            );
            continue;
        }

        // gem declarations
        if let Some(rest) = trimmed.strip_prefix("gem ") {
            let rest = rest.trim();
            // Parse gem name (first quoted string)
            let (name, version) = parse_gem_declaration(rest);
            if !name.is_empty() {
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    version,
                );
            }
        }
    }
}

fn parse_gem_declaration(s: &str) -> (String, Option<String>) {
    // Extract gem name from first quoted string
    let quote_char = if s.starts_with('\'') {
        '\''
    } else if s.starts_with('"') {
        '"'
    } else {
        return (String::new(), None);
    };

    let rest = &s[1..];
    let Some(end) = rest.find(quote_char) else {
        return (String::new(), None);
    };
    let name = rest[..end].to_string();

    // Look for version string (second quoted string)
    let after = &rest[end + 1..];
    let version = extract_quoted_string(after);

    (name, version)
}

fn extract_quoted_string(s: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        if let Some(start) = s.find(quote) {
            let rest = &s[start + 1..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

// ── composer.json ──

fn extract_composer_json(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(content) else {
        return;
    };

    // Package name
    if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
        let version = json.get("version").and_then(|v| v.as_str());
        let sig = version.map(|v| format!("{name}@{v}"));
        add_node(
            result,
            next_id,
            file_id,
            NodeKind::Module,
            name.to_string(),
            file_path,
            language,
            1,
            sig,
        );
    }

    // Dependencies
    for dep_key in &["require", "require-dev"] {
        if let Some(deps) = json.get(*dep_key).and_then(|v| v.as_object()) {
            for (name, version) in deps {
                let sig = version.as_str().map(|v| format!("{dep_key}: {v}"));
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name.clone(),
                    file_path,
                    language,
                    1,
                    sig,
                );
            }
        }
    }

    // Scripts
    if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
        for (name, cmd) in scripts {
            let sig = cmd.as_str().map(|c| c.to_string());
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Function,
                name.clone(),
                file_path,
                language,
                1,
                sig,
            );
        }
    }
}

// ── pom.xml (Maven) ──

fn extract_pom_xml(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    // Extract groupId, artifactId, version from top-level project element
    // We use simple string scanning to avoid an XML dependency
    let project_group = extract_xml_value(content, "groupId");
    let project_artifact = extract_xml_value(content, "artifactId");
    let project_version = extract_xml_value(content, "version");

    if let Some(ref artifact) = project_artifact {
        let name = match &project_group {
            Some(g) => format!("{g}:{artifact}"),
            None => artifact.clone(),
        };
        let sig = project_version.as_ref().map(|v| format!("{name}@{v}"));
        add_node(
            result,
            next_id,
            file_id,
            NodeKind::Module,
            name,
            file_path,
            language,
            1,
            sig,
        );
    }

    // Extract dependencies
    // Find all <dependency> blocks and extract groupId + artifactId + version
    let mut search_from = 0;
    while let Some(dep_start) = content[search_from..].find("<dependency>") {
        let dep_start = search_from + dep_start;
        let Some(dep_end) = content[dep_start..].find("</dependency>") else {
            break;
        };
        let dep_block = &content[dep_start..dep_start + dep_end];

        let group = extract_xml_value(dep_block, "groupId");
        let artifact = extract_xml_value(dep_block, "artifactId");
        let version = extract_xml_value(dep_block, "version");
        let scope = extract_xml_value(dep_block, "scope");

        if let Some(artifact) = artifact {
            let name = match &group {
                Some(g) => format!("{g}:{artifact}"),
                None => artifact,
            };
            let sig = match (&version, &scope) {
                (Some(v), Some(s)) => Some(format!("{s}: {v}")),
                (Some(v), None) => Some(v.clone()),
                (None, Some(s)) => Some(s.clone()),
                (None, None) => None,
            };
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Import,
                name,
                file_path,
                language,
                1,
                sig,
            );
        }

        search_from = dep_start + dep_end + "</dependency>".len();
    }

    // Extract plugins as well (useful for build context)
    let mut search_from = 0;
    while let Some(plugin_start) = content[search_from..].find("<plugin>") {
        let plugin_start = search_from + plugin_start;
        let Some(plugin_end) = content[plugin_start..].find("</plugin>") else {
            break;
        };
        let plugin_block = &content[plugin_start..plugin_start + plugin_end];

        let group = extract_xml_value(plugin_block, "groupId");
        let artifact = extract_xml_value(plugin_block, "artifactId");
        let version = extract_xml_value(plugin_block, "version");

        if let Some(artifact) = artifact {
            let name = match &group {
                Some(g) => format!("{g}:{artifact}"),
                None => artifact,
            };
            let sig = version.map(|v| format!("plugin: {v}"));
            add_node(
                result,
                next_id,
                file_id,
                NodeKind::Import,
                name,
                file_path,
                language,
                1,
                sig,
            );
        }

        search_from = plugin_start + plugin_end + "</plugin>".len();
    }
}

/// Extract a simple XML element value like <tag>value</tag>
fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let value_start = start + open.len();
    let end = xml[value_start..].find(&close)?;
    let value = xml[value_start..value_start + end].trim();
    if value.is_empty() || value.starts_with('<') {
        None
    } else {
        Some(value.to_string())
    }
}

// ── build.gradle / build.gradle.kts (Gradle) ──

fn extract_gradle(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }

        // Group/artifact from plugins block: id 'com.android.application' or id("org.jetbrains.kotlin.jvm")
        if trimmed.starts_with("id ") || trimmed.starts_with("id(") {
            let plugin = extract_gradle_string(trimmed);
            if let Some(plugin) = plugin {
                let version = extract_gradle_version(trimmed);
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    plugin,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    version.map(|v| format!("plugin: {v}")),
                );
            }
            continue;
        }

        // Dependency declarations: implementation 'group:artifact:version'
        // Also: api, compileOnly, runtimeOnly, testImplementation, etc.
        let dep_configs = [
            "implementation",
            "api",
            "compileOnly",
            "runtimeOnly",
            "testImplementation",
            "testCompileOnly",
            "testRuntimeOnly",
            "annotationProcessor",
            "kapt",
            "ksp",
            "classpath",
        ];

        let mut matched_config = None;
        for config in &dep_configs {
            if let Some(rest) = trimmed.strip_prefix(config) {
                if rest.starts_with('(')
                    || rest.starts_with(' ')
                    || rest.starts_with('"')
                    || rest.starts_with('\'')
                {
                    matched_config = Some(*config);
                    break;
                }
            }
        }

        if let Some(config) = matched_config {
            if let Some(dep) = extract_gradle_string(trimmed) {
                // dep might be "group:artifact:version" or just "artifact"
                let sig = Some(config.to_string());
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    dep,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    sig,
                );
            }
            continue;
        }

        // group = 'com.example' or group = "com.example"
        if trimmed.starts_with("group") && trimmed.contains('=') {
            if let Some(value) = extract_gradle_assignment(trimmed) {
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Module,
                    value,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    None,
                );
            }
        }
    }
}

/// Extract a quoted string from a Gradle line (first occurrence)
fn extract_gradle_string(s: &str) -> Option<String> {
    // Try double-quoted first, then single-quoted
    for quote in ['"', '\''] {
        if let Some(start) = s.find(quote) {
            let rest = &s[start + 1..];
            if let Some(end) = rest.find(quote) {
                let value = &rest[..end];
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Extract version from Gradle `version '...'` or `version "..."` in a line
fn extract_gradle_version(s: &str) -> Option<String> {
    let version_idx = s.find("version")?;
    let after = &s[version_idx + "version".len()..];
    extract_gradle_string(after)
}

/// Extract value from `key = 'value'` or `key = "value"` assignment
fn extract_gradle_assignment(s: &str) -> Option<String> {
    let eq_idx = s.find('=')?;
    let after = s[eq_idx + 1..].trim();
    // Strip quotes
    let after = after.trim_matches(|c| c == '\'' || c == '"');
    if after.is_empty() {
        None
    } else {
        Some(after.to_string())
    }
}

// ── build.sbt (SBT / Scala) ──

fn extract_build_sbt(
    content: &str,
    file_path: &str,
    language: Language,
    file_id: i64,
    next_id: &mut i64,
    result: &mut ExtractionResult,
) {
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }

        // name := "project-name"
        if trimmed.starts_with("name ") && trimmed.contains(":=") {
            if let Some(value) = extract_sbt_string_value(trimmed) {
                let version = find_sbt_setting(content, "version");
                let sig = version.map(|v| format!("{value}@{v}"));
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Module,
                    value,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    sig,
                );
            }
            continue;
        }

        // scalaVersion := "3.3.1"
        if trimmed.starts_with("scalaVersion ") && trimmed.contains(":=") {
            if let Some(value) = extract_sbt_string_value(trimmed) {
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Constant,
                    "scalaVersion".to_string(),
                    file_path,
                    language,
                    line_num as u32 + 1,
                    Some(value),
                );
            }
            continue;
        }

        // Library dependencies: "org.group" %% "artifact" % "version"
        // Also: "org.group" % "artifact" % "version" % "test"
        if trimmed.contains("%%") || (trimmed.contains('%') && trimmed.contains('"')) {
            if let Some((name, version, scope)) = parse_sbt_dependency(trimmed) {
                let sig = match (version, scope) {
                    (Some(v), Some(s)) => Some(format!("{s}: {v}")),
                    (Some(v), None) => Some(v),
                    (None, Some(s)) => Some(s),
                    (None, None) => None,
                };
                add_node(
                    result,
                    next_id,
                    file_id,
                    NodeKind::Import,
                    name,
                    file_path,
                    language,
                    line_num as u32 + 1,
                    sig,
                );
            }
        }
    }
}

fn extract_sbt_string_value(s: &str) -> Option<String> {
    let assign = s.find(":=")?;
    let after = s[assign + 2..].trim();
    let after = after.trim_matches('"');
    if after.is_empty() {
        None
    } else {
        Some(after.to_string())
    }
}

fn find_sbt_setting(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(key) && trimmed.contains(":=") {
            return extract_sbt_string_value(trimmed);
        }
    }
    None
}

/// Parse SBT dependency: "org" %% "artifact" % "version" [% "scope"]
fn parse_sbt_dependency(s: &str) -> Option<(String, Option<String>, Option<String>)> {
    // Extract all quoted strings from the line
    let strings: Vec<&str> = extract_all_quoted_strings(s);

    match strings.len() {
        // "org" %% "artifact" % "version" % "scope"
        4 => {
            let name = format!("{}:{}", strings[0], strings[1]);
            Some((
                name,
                Some(strings[2].to_string()),
                Some(strings[3].to_string()),
            ))
        }
        // "org" %% "artifact" % "version"
        3 => {
            let name = format!("{}:{}", strings[0], strings[1]);
            Some((name, Some(strings[2].to_string()), None))
        }
        // "org" %% "artifact" (no version)
        2 => {
            let name = format!("{}:{}", strings[0], strings[1]);
            Some((name, None, None))
        }
        _ => None,
    }
}

fn extract_all_quoted_strings(s: &str) -> Vec<&str> {
    let mut results = Vec::new();
    let mut remaining = s;
    while let Some(start) = remaining.find('"') {
        remaining = &remaining[start + 1..];
        if let Some(end) = remaining.find('"') {
            let value = &remaining[..end];
            if !value.is_empty() {
                results.push(value);
            }
            remaining = &remaining[end + 1..];
        } else {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_manifest_file() {
        assert!(is_manifest_file("package.json"));
        assert!(is_manifest_file("Cargo.toml"));
        assert!(is_manifest_file("go.mod"));
        assert!(is_manifest_file("pyproject.toml"));
        assert!(is_manifest_file("requirements.txt"));
        assert!(is_manifest_file("Gemfile"));
        assert!(is_manifest_file("composer.json"));
        assert!(is_manifest_file("pom.xml"));
        assert!(is_manifest_file("build.gradle"));
        assert!(is_manifest_file("build.gradle.kts"));
        assert!(is_manifest_file("build.sbt"));
        assert!(!is_manifest_file("main.rs"));
        assert!(!is_manifest_file("config.json"));
    }

    #[test]
    fn test_package_json() {
        let content = r#"{
  "name": "my-app",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0",
    "lodash": "^4.17.21"
  },
  "devDependencies": {
    "jest": "^29.0.0"
  },
  "scripts": {
    "start": "node index.js",
    "test": "jest"
  }
}"#;
        let result = extract_manifest("package.json", content);
        assert!(result.errors.is_empty());

        // File node + module + 3 deps + 2 scripts = 7
        assert_eq!(result.nodes.len(), 7);

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "my-app");
        assert_eq!(module.signature.as_deref(), Some("my-app@1.0.0"));

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "express"));
        assert!(imports.iter().any(|n| n.name == "lodash"));
        assert!(imports.iter().any(|n| n.name == "jest"));

        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Function)
            .collect();
        assert_eq!(funcs.len(), 2);
        assert!(funcs.iter().any(|n| n.name == "start"));
        assert!(funcs.iter().any(|n| n.name == "test"));
    }

    #[test]
    fn test_cargo_toml() {
        let content = r#"
[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = "1"

[dev-dependencies]
tempfile = "3"

[features]
default = ["sqlite"]
sqlite = []
"#;
        let result = extract_manifest("Cargo.toml", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "my-crate");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "serde"));
        assert!(imports.iter().any(|n| n.name == "tokio"));
        assert!(imports.iter().any(|n| n.name == "tempfile"));

        let features: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Constant)
            .collect();
        assert_eq!(features.len(), 2);
        assert!(features.iter().any(|n| n.name == "default"));
        assert!(features.iter().any(|n| n.name == "sqlite"));
    }

    #[test]
    fn test_go_mod() {
        let content = r#"module github.com/example/mymod

go 1.21

require (
	github.com/foo/bar v1.2.3
	github.com/baz/qux v0.1.0
)

require github.com/single/dep v2.0.0
"#;
        let result = extract_manifest("go.mod", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "github.com/example/mymod");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "github.com/foo/bar"));
        assert!(imports.iter().any(|n| n.name == "github.com/baz/qux"));
        assert!(imports.iter().any(|n| n.name == "github.com/single/dep"));
    }

    #[test]
    fn test_pyproject_toml_pep621() {
        let content = r#"
[project]
name = "my-package"
version = "0.1.0"
dependencies = [
    "requests>=2.25.0",
    "click~=8.0",
    "numpy",
]

[project.optional-dependencies]
dev = ["pytest>=7.0"]
"#;
        let result = extract_manifest("pyproject.toml", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "my-package");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 4);
        assert!(imports.iter().any(|n| n.name == "requests"));
        assert!(imports.iter().any(|n| n.name == "click"));
        assert!(imports.iter().any(|n| n.name == "numpy"));
        assert!(imports.iter().any(|n| n.name == "pytest"));
    }

    #[test]
    fn test_pyproject_toml_poetry() {
        let content = r#"
[tool.poetry]
name = "my-poetry-project"
version = "1.0.0"

[tool.poetry.dependencies]
python = "^3.9"
flask = "^2.0"
sqlalchemy = { version = "^1.4", optional = true }

[tool.poetry.dev-dependencies]
black = "^22.0"
"#;
        let result = extract_manifest("pyproject.toml", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "my-poetry-project");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        // flask, sqlalchemy, black (python is skipped)
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "flask"));
        assert!(imports.iter().any(|n| n.name == "sqlalchemy"));
        assert!(imports.iter().any(|n| n.name == "black"));
    }

    #[test]
    fn test_requirements_txt() {
        let content = r#"
# Core dependencies
flask==2.0.0
requests>=2.25.0
numpy
# Dev tools
-e .
--index-url https://pypi.org/simple
pytest>=7.0
"#;
        let result = extract_manifest("requirements.txt", content);
        assert!(result.errors.is_empty());

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 4);
        assert!(imports.iter().any(|n| n.name == "flask"));
        assert!(imports.iter().any(|n| n.name == "requests"));
        assert!(imports.iter().any(|n| n.name == "numpy"));
        assert!(imports.iter().any(|n| n.name == "pytest"));
    }

    #[test]
    fn test_gemfile() {
        let content = r#"
source 'https://rubygems.org'

ruby '3.2.0'

gem 'rails', '~> 7.0'
gem 'pg'
gem 'puma', '>= 5.0'
"#;
        let result = extract_manifest("Gemfile", content);
        assert!(result.errors.is_empty());

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "rails"));
        assert!(imports.iter().any(|n| n.name == "pg"));
        assert!(imports.iter().any(|n| n.name == "puma"));

        let consts: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Constant)
            .collect();
        assert!(consts.iter().any(|n| n.name == "source"));
        assert!(consts.iter().any(|n| n.name == "ruby"));
    }

    #[test]
    fn test_composer_json() {
        let content = r#"{
  "name": "vendor/my-package",
  "require": {
    "php": "^8.1",
    "laravel/framework": "^10.0"
  },
  "require-dev": {
    "phpunit/phpunit": "^10.0"
  },
  "scripts": {
    "test": "phpunit"
  }
}"#;
        let result = extract_manifest("composer.json", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "vendor/my-package");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);

        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Function)
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "test");
    }

    #[test]
    fn test_manifest_language() {
        assert_eq!(manifest_language("package.json"), Language::JavaScript);
        assert_eq!(manifest_language("Cargo.toml"), Language::Rust);
        assert_eq!(manifest_language("go.mod"), Language::Go);
        assert_eq!(manifest_language("pyproject.toml"), Language::Python);
        assert_eq!(manifest_language("requirements.txt"), Language::Python);
        assert_eq!(manifest_language("Gemfile"), Language::Ruby);
        assert_eq!(manifest_language("composer.json"), Language::Php);
        assert_eq!(manifest_language("pom.xml"), Language::Java);
        assert_eq!(manifest_language("build.gradle"), Language::Groovy);
        assert_eq!(manifest_language("build.gradle.kts"), Language::Kotlin);
        assert_eq!(manifest_language("build.sbt"), Language::Scala);
    }

    #[test]
    fn test_parse_pep508_dependency() {
        assert_eq!(
            parse_pep508_dependency("requests>=2.25.0"),
            ("requests".to_string(), Some(">=2.25.0".to_string()))
        );
        assert_eq!(
            parse_pep508_dependency("numpy"),
            ("numpy".to_string(), None)
        );
        assert_eq!(
            parse_pep508_dependency("click~=8.0"),
            ("click".to_string(), Some("~=8.0".to_string()))
        );
    }

    #[test]
    fn test_invalid_json_returns_file_node_only() {
        let result = extract_manifest("package.json", "not valid json");
        assert!(result.errors.is_empty());
        assert_eq!(result.nodes.len(), 1); // just the file node
        assert_eq!(result.nodes[0].kind, NodeKind::File);
    }

    #[test]
    fn test_invalid_toml_returns_file_node_only() {
        let result = extract_manifest("Cargo.toml", "not valid toml [[[");
        assert!(result.errors.is_empty());
        assert_eq!(result.nodes.len(), 1);
    }

    #[test]
    fn test_contains_edges_created() {
        let content = r#"{"name": "test", "dependencies": {"foo": "^1.0"}}"#;
        let result = extract_manifest("package.json", content);

        let contains_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .collect();
        // Module node + Import node = 2 contains edges from file
        assert_eq!(contains_edges.len(), 2);
    }

    #[test]
    fn test_pom_xml() {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
    <groupId>com.example</groupId>
    <artifactId>my-app</artifactId>
    <version>1.0.0</version>
    <packaging>jar</packaging>

    <dependencies>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-starter-web</artifactId>
            <version>3.1.0</version>
        </dependency>
        <dependency>
            <groupId>junit</groupId>
            <artifactId>junit</artifactId>
            <version>4.13.2</version>
            <scope>test</scope>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-compiler-plugin</artifactId>
                <version>3.11.0</version>
            </plugin>
        </plugins>
    </build>
</project>"#;
        let result = extract_manifest("pom.xml", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "com.example:my-app");
        assert_eq!(
            module.signature.as_deref(),
            Some("com.example:my-app@1.0.0")
        );

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3); // 2 deps + 1 plugin
        assert!(imports
            .iter()
            .any(|n| n.name == "org.springframework.boot:spring-boot-starter-web"));
        assert!(imports.iter().any(|n| n.name == "junit:junit"));
        assert!(imports
            .iter()
            .any(|n| n.name == "org.apache.maven.plugins:maven-compiler-plugin"));

        // Check scope is captured
        let junit = imports.iter().find(|n| n.name == "junit:junit").unwrap();
        assert!(junit.signature.as_deref().unwrap().contains("test"));
    }

    #[test]
    fn test_build_gradle() {
        let content = r#"
plugins {
    id 'java'
    id 'org.springframework.boot' version '3.1.0'
}

group = 'com.example'
version = '1.0.0'

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web:3.1.0'
    testImplementation 'junit:junit:4.13.2'
    runtimeOnly 'org.postgresql:postgresql:42.6.0'
}
"#;
        let result = extract_manifest("build.gradle", content);
        assert!(result.errors.is_empty());

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "com.example");

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        // 2 plugins + 3 dependencies = 5
        assert_eq!(imports.len(), 5);
        assert!(imports
            .iter()
            .any(|n| n.name == "org.springframework.boot:spring-boot-starter-web:3.1.0"));
        assert!(imports.iter().any(|n| n.name == "junit:junit:4.13.2"));
        assert!(imports
            .iter()
            .any(|n| n.name == "org.postgresql:postgresql:42.6.0"));
        assert!(imports.iter().any(|n| n.name == "java"));
        assert!(imports.iter().any(|n| n.name == "org.springframework.boot"));
    }

    #[test]
    fn test_build_gradle_kts() {
        let content = r#"
plugins {
    id("org.jetbrains.kotlin.jvm") version "1.9.0"
    id("application")
}

group = "com.example"
version = "1.0.0"

dependencies {
    implementation("io.ktor:ktor-server-core:2.3.0")
    testImplementation("org.jetbrains.kotlin:kotlin-test")
}
"#;
        let result = extract_manifest("build.gradle.kts", content);
        assert!(result.errors.is_empty());
        assert_eq!(result.nodes[0].language, Language::Kotlin);

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert!(imports
            .iter()
            .any(|n| n.name == "io.ktor:ktor-server-core:2.3.0"));
        assert!(imports
            .iter()
            .any(|n| n.name == "org.jetbrains.kotlin:kotlin-test"));
        assert!(imports.iter().any(|n| n.name == "org.jetbrains.kotlin.jvm"));
        assert!(imports.iter().any(|n| n.name == "application"));
    }

    #[test]
    fn test_build_sbt() {
        let content = r#"
name := "my-scala-app"
version := "0.1.0"
scalaVersion := "3.3.1"

libraryDependencies += "org.typelevel" %% "cats-core" % "2.10.0"
libraryDependencies += "org.scalatest" %% "scalatest" % "3.2.17" % "test"
libraryDependencies += "com.typesafe" % "config" % "1.4.3"
"#;
        let result = extract_manifest("build.sbt", content);
        assert!(result.errors.is_empty());
        assert_eq!(result.nodes[0].language, Language::Scala);

        let module = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap();
        assert_eq!(module.name, "my-scala-app");
        assert_eq!(module.signature.as_deref(), Some("my-scala-app@0.1.0"));

        let scala_version = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Constant && n.name == "scalaVersion")
            .unwrap();
        assert_eq!(scala_version.signature.as_deref(), Some("3.3.1"));

        let imports: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|n| n.name == "org.typelevel:cats-core"));
        assert!(imports.iter().any(|n| n.name == "org.scalatest:scalatest"));
        assert!(imports.iter().any(|n| n.name == "com.typesafe:config"));

        // Check scope is captured for test dependency
        let scalatest = imports
            .iter()
            .find(|n| n.name == "org.scalatest:scalatest")
            .unwrap();
        assert!(scalatest.signature.as_deref().unwrap().contains("test"));
    }
}

//! Language-specific configurations for tree-sitter extraction

use tree_sitter::Language as TsLanguage;

use crate::types::{Language, NodeKind};

/// Language-specific configuration for extraction
pub struct LanguageConfig {
    /// Node types that map to functions
    pub function_types: &'static [&'static str],
    /// Node types that map to methods
    pub method_types: &'static [&'static str],
    /// Node types that map to classes
    pub class_types: &'static [&'static str],
    /// Node types that map to structs
    pub struct_types: &'static [&'static str],
    /// Node types that map to interfaces/traits
    pub interface_types: &'static [&'static str],
    /// Node types that map to enums
    pub enum_types: &'static [&'static str],
    /// Node types that map to imports
    pub import_types: &'static [&'static str],
    /// Node types that represent function calls
    pub call_types: &'static [&'static str],
    /// Node types that map to type aliases
    pub type_alias_types: &'static [&'static str],
    /// Node types that map to constants
    pub constant_types: &'static [&'static str],
    /// Node types that map to variables (reserved for future use)
    #[allow(dead_code)]
    pub variable_types: &'static [&'static str],
    /// Node types that map to modules/namespaces
    pub module_types: &'static [&'static str],
}

impl LanguageConfig {
    /// Convert a tree-sitter node type to our NodeKind
    pub fn node_type_to_kind(&self, node_type: &str) -> Option<NodeKind> {
        if self.function_types.contains(&node_type) {
            return Some(NodeKind::Function);
        }
        if self.method_types.contains(&node_type) {
            return Some(NodeKind::Method);
        }
        if self.class_types.contains(&node_type) {
            return Some(NodeKind::Class);
        }
        if self.struct_types.contains(&node_type) {
            return Some(NodeKind::Struct);
        }
        if self.interface_types.contains(&node_type) {
            return Some(NodeKind::Interface);
        }
        if self.enum_types.contains(&node_type) {
            return Some(NodeKind::Enum);
        }
        if self.import_types.contains(&node_type) {
            return Some(NodeKind::Import);
        }
        if self.type_alias_types.contains(&node_type) {
            return Some(NodeKind::TypeAlias);
        }
        if self.constant_types.contains(&node_type) {
            return Some(NodeKind::Constant);
        }
        if self.module_types.contains(&node_type) {
            return Some(NodeKind::Module);
        }
        None
    }

    /// Check if a node type represents a function call
    pub fn is_call_node(&self, node_type: &str) -> bool {
        self.call_types.contains(&node_type)
    }
}

/// Get the tree-sitter language for a given Language
pub fn get_language(lang: Language) -> Option<TsLanguage> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        Language::TypeScript | Language::Tsx => {
            Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        }
        Language::JavaScript | Language::Jsx => Some(tree_sitter_javascript::LANGUAGE.into()),
        Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
        Language::Go => Some(tree_sitter_go::LANGUAGE.into()),
        Language::Java => Some(tree_sitter_java::LANGUAGE.into()),
        Language::C => Some(tree_sitter_c::LANGUAGE.into()),
        Language::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
        Language::CSharp => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        Language::Kotlin => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        Language::Scala => Some(tree_sitter_scala::LANGUAGE.into()),
        Language::Groovy => Some(tree_sitter_groovy::LANGUAGE.into()),
        _ => None,
    }
}

/// Get the language configuration for a given Language
pub fn get_config(lang: Language) -> &'static LanguageConfig {
    match lang {
        Language::Rust => &RUST_CONFIG,
        Language::TypeScript | Language::Tsx => &TYPESCRIPT_CONFIG,
        Language::JavaScript | Language::Jsx => &JAVASCRIPT_CONFIG,
        Language::Python => &PYTHON_CONFIG,
        Language::Go => &GO_CONFIG,
        Language::Java => &JAVA_CONFIG,
        Language::C => &C_CONFIG,
        Language::Cpp => &CPP_CONFIG,
        Language::CSharp => &CSHARP_CONFIG,
        Language::Kotlin => &KOTLIN_CONFIG,
        Language::Scala => &SCALA_CONFIG,
        Language::Groovy => &GROOVY_CONFIG,
        _ => &DEFAULT_CONFIG,
    }
}

static DEFAULT_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &[],
    method_types: &[],
    class_types: &[],
    struct_types: &[],
    interface_types: &[],
    enum_types: &[],
    import_types: &[],
    call_types: &[],
    type_alias_types: &[],
    constant_types: &[],
    variable_types: &[],
    module_types: &[],
};

static RUST_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_item"],
    method_types: &["function_item"], // Methods are also function_item in impl blocks
    class_types: &[],
    struct_types: &["struct_item"],
    interface_types: &["trait_item"],
    enum_types: &["enum_item"],
    import_types: &["use_declaration"],
    call_types: &["call_expression", "macro_invocation"],
    type_alias_types: &["type_item"],
    constant_types: &["const_item", "static_item"],
    variable_types: &["let_declaration"],
    module_types: &["mod_item"],
};

static TYPESCRIPT_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &[
        "function_declaration",
        "arrow_function",
        "function_expression",
        "generator_function_declaration",
    ],
    method_types: &["method_definition", "method_signature"],
    class_types: &["class_declaration", "class"],
    struct_types: &[],
    interface_types: &["interface_declaration"],
    enum_types: &["enum_declaration"],
    import_types: &["import_statement", "import_clause"],
    call_types: &["call_expression", "new_expression"],
    type_alias_types: &["type_alias_declaration"],
    constant_types: &[],
    variable_types: &["variable_declaration", "lexical_declaration"],
    module_types: &["module", "namespace_declaration"],
};

static JAVASCRIPT_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &[
        "function_declaration",
        "arrow_function",
        "function_expression",
        "generator_function_declaration",
    ],
    method_types: &["method_definition"],
    class_types: &["class_declaration", "class"],
    struct_types: &[],
    interface_types: &[],
    enum_types: &[],
    import_types: &["import_statement"],
    call_types: &["call_expression", "new_expression"],
    type_alias_types: &[],
    constant_types: &[],
    variable_types: &["variable_declaration", "lexical_declaration"],
    module_types: &[],
};

static PYTHON_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_definition"],
    method_types: &[], // Python methods are function_definition inside class
    class_types: &["class_definition"],
    struct_types: &[],
    interface_types: &[],
    enum_types: &[],
    import_types: &["import_statement", "import_from_statement"],
    call_types: &["call"],
    type_alias_types: &[],
    constant_types: &[],
    variable_types: &["assignment"],
    module_types: &[],
};

static GO_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_declaration"],
    method_types: &["method_declaration"],
    class_types: &[],
    struct_types: &["type_declaration"], // Go structs are type declarations
    interface_types: &["type_declaration"], // Go interfaces too
    enum_types: &[],
    import_types: &["import_declaration", "import_spec"],
    call_types: &["call_expression"],
    type_alias_types: &["type_alias"],
    constant_types: &["const_declaration"],
    variable_types: &["var_declaration", "short_var_declaration"],
    module_types: &["package_clause"],
};

static JAVA_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &[],
    method_types: &["method_declaration", "constructor_declaration"],
    class_types: &["class_declaration"],
    struct_types: &[],
    interface_types: &["interface_declaration"],
    enum_types: &["enum_declaration"],
    import_types: &["import_declaration"],
    call_types: &["method_invocation", "object_creation_expression"],
    type_alias_types: &[],
    constant_types: &["field_declaration"],
    variable_types: &["local_variable_declaration"],
    module_types: &["package_declaration"],
};

static C_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_definition"],
    method_types: &[],
    class_types: &[],
    struct_types: &["struct_specifier"],
    interface_types: &[],
    enum_types: &["enum_specifier"],
    import_types: &["preproc_include"],
    call_types: &["call_expression"],
    type_alias_types: &["type_definition"],
    constant_types: &["preproc_def"],
    variable_types: &["declaration"],
    module_types: &[],
};

static CPP_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_definition"],
    method_types: &["function_definition"], // Methods are function_definition inside class
    class_types: &["class_specifier"],
    struct_types: &["struct_specifier"],
    interface_types: &[],
    enum_types: &["enum_specifier"],
    import_types: &["preproc_include"],
    call_types: &["call_expression"],
    type_alias_types: &["type_definition", "alias_declaration"],
    constant_types: &["preproc_def"],
    variable_types: &["declaration"],
    module_types: &["namespace_definition"],
};

static CSHARP_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &[],
    method_types: &[
        "method_declaration",
        "constructor_declaration",
        "destructor_declaration",
        "operator_declaration",
        "conversion_operator_declaration",
    ],
    class_types: &["class_declaration", "record_declaration"],
    struct_types: &["struct_declaration"],
    interface_types: &["interface_declaration"],
    enum_types: &["enum_declaration"],
    import_types: &["using_directive"],
    call_types: &["invocation_expression", "object_creation_expression"],
    type_alias_types: &["delegate_declaration"],
    constant_types: &["field_declaration"], // C# constants are field_declaration with const modifier
    variable_types: &["variable_declaration"],
    module_types: &["namespace_declaration"],
};

static KOTLIN_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_declaration", "anonymous_function"],
    method_types: &["function_declaration"], // Methods are function_declaration inside class
    class_types: &["class_declaration", "object_declaration"],
    struct_types: &[],
    interface_types: &["class_declaration"], // Interfaces use class_declaration with interface modifier
    enum_types: &["class_declaration"],      // Enums use class_declaration with enum modifier
    import_types: &["import"],
    call_types: &["call_expression", "constructor_invocation"],
    type_alias_types: &["type_alias"],
    constant_types: &["property_declaration"],
    variable_types: &["property_declaration"],
    module_types: &["package_header", "companion_object"],
};

static SCALA_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_definition", "function_declaration"],
    method_types: &["function_definition", "function_declaration"],
    class_types: &["class_definition", "object_definition"],
    struct_types: &[],
    interface_types: &["trait_definition"],
    enum_types: &["enum_definition"],
    import_types: &["import_declaration"],
    call_types: &["call_expression"],
    type_alias_types: &["type_definition"],
    constant_types: &["val_definition", "val_declaration"],
    variable_types: &["var_definition", "var_declaration"],
    module_types: &["package_clause", "package_object"],
};

static GROOVY_CONFIG: LanguageConfig = LanguageConfig {
    function_types: &["function_definition"],
    method_types: &["method_declaration", "constructor_declaration"],
    class_types: &["class_declaration"],
    struct_types: &[],
    interface_types: &["interface_declaration"],
    enum_types: &["enum_declaration"],
    import_types: &["import_declaration"],
    call_types: &["method_invocation", "juxt_function_call", "object_creation_expression"],
    type_alias_types: &[],
    constant_types: &["constant_declaration", "field_declaration"],
    variable_types: &["local_variable_declaration"],
    module_types: &["package_declaration"],
};

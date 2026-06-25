//! Product-neutral mail template registration and rendering helpers.
//!
//! Products still own template codes, default subject/body content, runtime configuration keys,
//! payload types, URLs, and localization. This module only provides the shared mechanics around a
//! registered template catalog: variable metadata, placeholder substitution, HTML escaping, and
//! text fallback generation.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

/// Rendered mail bodies produced from a template and placeholder values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMail {
    /// Rendered message subject.
    pub subject: String,
    /// Plain-text fallback body.
    pub text_body: String,
    /// Rendered HTML body.
    pub html_body: String,
}

/// Variable metadata exposed to product admin UIs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct TemplateVariableItem {
    /// Placeholder token displayed in UI, such as `{{username}}`.
    pub token: String,
    /// Product-owned i18n label key.
    pub label_i18n_key: String,
    /// Product-owned i18n description key.
    pub description_i18n_key: String,
}

/// Variable metadata for one registered template.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct TemplateVariableGroup {
    /// Product-owned configuration category.
    pub category: String,
    /// Stable product template code.
    pub template_code: String,
    /// Product-owned i18n group label key.
    pub label_i18n_key: String,
    /// Variables accepted by the template.
    pub variables: Vec<TemplateVariableItem>,
}

/// One placeholder accepted by a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateVariableSpec {
    /// Placeholder key without braces.
    pub key: &'static str,
    /// Product-owned i18n label key.
    pub label_i18n_key: &'static str,
    /// Product-owned i18n description key.
    pub description_i18n_key: &'static str,
}

impl TemplateVariableSpec {
    /// Creates a variable spec.
    pub const fn new(
        key: &'static str,
        label_i18n_key: &'static str,
        description_i18n_key: &'static str,
    ) -> Self {
        Self {
            key,
            label_i18n_key,
            description_i18n_key,
        }
    }
}

/// Registered metadata for one product mail template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailTemplateDefinition {
    /// Stable product template code.
    pub code: &'static str,
    /// Product-owned configuration category.
    pub category: &'static str,
    /// Product-owned i18n group label key.
    pub label_i18n_key: &'static str,
    /// Variables accepted by the template.
    pub variables: &'static [TemplateVariableSpec],
}

impl MailTemplateDefinition {
    /// Creates a registered template definition.
    pub const fn new(
        code: &'static str,
        category: &'static str,
        label_i18n_key: &'static str,
        variables: &'static [TemplateVariableSpec],
    ) -> Self {
        Self {
            code,
            category,
            label_i18n_key,
            variables,
        }
    }

    /// Converts this definition into API-facing variable metadata.
    pub fn variable_group(&self) -> TemplateVariableGroup {
        TemplateVariableGroup {
            category: self.category.to_string(),
            template_code: self.code.to_string(),
            label_i18n_key: self.label_i18n_key.to_string(),
            variables: self
                .variables
                .iter()
                .map(|variable| TemplateVariableItem {
                    token: format!("{{{{{}}}}}", variable.key),
                    label_i18n_key: variable.label_i18n_key.to_string(),
                    description_i18n_key: variable.description_i18n_key.to_string(),
                })
                .collect(),
        }
    }
}

/// A product-owned template registry.
#[derive(Debug, Clone, Copy)]
pub struct MailTemplateRegistry {
    definitions: &'static [MailTemplateDefinition],
}

impl MailTemplateRegistry {
    /// Creates a registry from static product definitions.
    pub const fn new(definitions: &'static [MailTemplateDefinition]) -> Self {
        Self { definitions }
    }

    /// Returns registered definitions in product order.
    pub const fn definitions(&self) -> &'static [MailTemplateDefinition] {
        self.definitions
    }

    /// Returns variable groups in product registration order.
    pub fn variable_groups(&self) -> Vec<TemplateVariableGroup> {
        self.definitions
            .iter()
            .map(MailTemplateDefinition::variable_group)
            .collect()
    }

    /// Looks up a template definition by code.
    pub fn get(&self, code: &str) -> Option<&'static MailTemplateDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.code == code)
    }

    /// Validates that this static registry has unique template codes and variable keys.
    pub fn validate(&self) -> Result<(), MailTemplateRegistryError> {
        validate_definitions(self.definitions.iter())
    }
}

/// Runtime-composed template registry built from product and subsystem registrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailTemplateCatalog {
    definitions: Vec<&'static MailTemplateDefinition>,
}

impl MailTemplateCatalog {
    /// Creates an empty catalog builder.
    pub fn builder() -> MailTemplateCatalogBuilder {
        MailTemplateCatalogBuilder::new()
    }

    /// Returns registered definitions in registration order.
    pub fn definitions(&self) -> &[&'static MailTemplateDefinition] {
        &self.definitions
    }

    /// Returns variable groups in registration order.
    pub fn variable_groups(&self) -> Vec<TemplateVariableGroup> {
        self.definitions
            .iter()
            .map(|definition| definition.variable_group())
            .collect()
    }

    /// Looks up a template definition by code.
    pub fn get(&self, code: &str) -> Option<&'static MailTemplateDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| definition.code == code)
    }
}

/// Builder used by products to assemble a mail template catalog from multiple subsystems.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailTemplateCatalogBuilder {
    definitions: Vec<&'static MailTemplateDefinition>,
}

impl MailTemplateCatalogBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one template definition.
    pub fn register(&mut self, definition: &'static MailTemplateDefinition) -> &mut Self {
        self.definitions.push(definition);
        self
    }

    /// Registers all definitions from a static slice.
    pub fn register_all(&mut self, definitions: &'static [MailTemplateDefinition]) -> &mut Self {
        self.definitions.extend(definitions.iter());
        self
    }

    /// Builds a catalog after validating duplicate template codes and variable keys.
    pub fn build(self) -> Result<MailTemplateCatalog, MailTemplateRegistryError> {
        validate_definitions(self.definitions.iter().copied())?;
        Ok(MailTemplateCatalog {
            definitions: self.definitions,
        })
    }
}

/// Validation error returned when a template registry has ambiguous registrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailTemplateRegistryError {
    /// A template code is empty.
    EmptyTemplateCode,
    /// The same template code was registered more than once.
    DuplicateTemplateCode {
        /// Duplicated template code.
        code: &'static str,
    },
    /// A variable key is empty for a template.
    EmptyVariableKey {
        /// Template code containing the empty variable key.
        template_code: &'static str,
    },
    /// The same variable key was registered more than once for one template.
    DuplicateVariableKey {
        /// Template code containing the duplicated variable key.
        template_code: &'static str,
        /// Duplicated variable key.
        key: &'static str,
    },
}

impl fmt::Display for MailTemplateRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTemplateCode => formatter.write_str("mail template code must not be empty"),
            Self::DuplicateTemplateCode { code } => {
                write!(
                    formatter,
                    "mail template code `{code}` is registered more than once"
                )
            }
            Self::EmptyVariableKey { template_code } => write!(
                formatter,
                "mail template `{template_code}` contains an empty variable key"
            ),
            Self::DuplicateVariableKey { template_code, key } => write!(
                formatter,
                "mail template `{template_code}` registers variable `{key}` more than once"
            ),
        }
    }
}

impl Error for MailTemplateRegistryError {}

fn validate_definitions<'a, I>(definitions: I) -> Result<(), MailTemplateRegistryError>
where
    I: IntoIterator<Item = &'a MailTemplateDefinition>,
{
    let mut template_codes = HashSet::new();

    for definition in definitions {
        if definition.code.is_empty() {
            return Err(MailTemplateRegistryError::EmptyTemplateCode);
        }
        if !template_codes.insert(definition.code) {
            return Err(MailTemplateRegistryError::DuplicateTemplateCode {
                code: definition.code,
            });
        }

        let mut variable_keys = HashSet::new();
        for variable in definition.variables {
            if variable.key.is_empty() {
                return Err(MailTemplateRegistryError::EmptyVariableKey {
                    template_code: definition.code,
                });
            }
            if !variable_keys.insert(variable.key) {
                return Err(MailTemplateRegistryError::DuplicateVariableKey {
                    template_code: definition.code,
                    key: variable.key,
                });
            }
        }
    }

    Ok(())
}

/// Placeholder values for a render pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePlaceholderSet {
    text_values: Vec<(&'static str, String)>,
    html_values: Vec<(&'static str, String)>,
}

impl TemplatePlaceholderSet {
    /// Creates a placeholder set from separate text and HTML values.
    pub fn new(
        text_values: Vec<(&'static str, String)>,
        html_values: Vec<(&'static str, String)>,
    ) -> Self {
        Self {
            text_values,
            html_values,
        }
    }

    /// Returns placeholder values for plain text and subject rendering.
    pub fn text_values(&self) -> &[(&'static str, String)] {
        &self.text_values
    }

    /// Returns placeholder values for HTML rendering.
    pub fn html_values(&self) -> &[(&'static str, String)] {
        &self.html_values
    }
}

/// Renders a subject and HTML template with placeholder values and derives text fallback.
pub fn render_template(
    subject_template: String,
    html_template: String,
    placeholders: &TemplatePlaceholderSet,
) -> RenderedMail {
    let subject = render_placeholders(subject_template, placeholders.text_values());
    let html_body = render_placeholders(html_template, placeholders.html_values());
    let text_body = html_to_text(&html_body);

    RenderedMail {
        subject,
        text_body,
        html_body,
    }
}

/// Replaces `{{key}}` placeholders with provided values.
pub fn render_placeholders(mut template: String, values: &[(&'static str, String)]) -> String {
    for (key, value) in values {
        let placeholder = format!("{{{{{key}}}}}");
        template = template.replace(&placeholder, value);
    }
    template
}

/// Escapes text for insertion into HTML templates.
pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Converts simple HTML email content into a plain-text fallback.
pub fn html_to_text(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut ignored_tags = Vec::new();

    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                if let Some(parsed_tag) = parse_tag(&tag) {
                    if ignored_tags.is_empty() {
                        apply_tag_to_text(&mut output, &parsed_tag);
                    }
                    update_ignored_tags(&mut ignored_tags, &parsed_tag);
                }
                tag.clear();
                in_tag = false;
            } else {
                tag.push(ch);
            }
            continue;
        }

        if ch == '<' {
            in_tag = true;
            continue;
        }

        if ignored_tags.is_empty() {
            output.push(ch);
        }
    }

    let decoded = decode_html_entities(&output);
    normalize_text_fallback(&decoded)
}

fn apply_tag_to_text(output: &mut String, tag: &ParsedTag) {
    if tag.is_closing {
        return;
    }

    if tag.name == "li" && !output.ends_with("- ") {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("- ");
        return;
    }

    let needs_newline = matches!(
        tag.name.as_str(),
        "p" | "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "tr"
            | "table"
            | "br"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    );

    if needs_newline && !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn parse_tag(tag: &str) -> Option<ParsedTag> {
    let trimmed = tag.trim();
    if trimmed.is_empty() || trimmed.starts_with('!') || trimmed.starts_with('?') {
        return None;
    }

    let is_closing = trimmed.starts_with('/');
    let content = if is_closing { &trimmed[1..] } else { trimmed };
    let is_self_closing = content.ends_with('/');
    let name = content
        .trim_end_matches('/')
        .split_whitespace()
        .next()?
        .to_ascii_lowercase();

    Some(ParsedTag {
        name,
        is_closing,
        is_self_closing,
    })
}

fn update_ignored_tags(ignored_tags: &mut Vec<String>, tag: &ParsedTag) {
    if !is_ignored_text_tag(&tag.name) || tag.is_self_closing {
        return;
    }

    if tag.is_closing {
        if ignored_tags.last().is_some_and(|name| name == &tag.name) {
            ignored_tags.pop();
        }
        return;
    }

    ignored_tags.push(tag.name.clone());
}

fn is_ignored_text_tag(name: &str) -> bool {
    matches!(name, "head" | "script" | "style" | "title")
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_text_fallback(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_blank = true;

    for line in value.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !last_blank {
                normalized.push('\n');
            }
            last_blank = true;
            continue;
        }

        if !normalized.is_empty() && !normalized.ends_with('\n') {
            normalized.push('\n');
        }
        normalized.push_str(trimmed);
        last_blank = false;
    }

    normalized.trim().to_string()
}

struct ParsedTag {
    name: String,
    is_closing: bool,
    is_self_closing: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        MailTemplateCatalog, MailTemplateDefinition, MailTemplateRegistry,
        MailTemplateRegistryError, TemplatePlaceholderSet, TemplateVariableSpec, escape_html,
        html_to_text, render_template,
    };

    const VARIABLES: &[TemplateVariableSpec] = &[
        TemplateVariableSpec::new("username", "username_label", "username_desc"),
        TemplateVariableSpec::new("site_name", "site_name_label", "site_name_desc"),
    ];
    const DEFINITIONS: &[MailTemplateDefinition] = &[MailTemplateDefinition::new(
        "welcome",
        "mail_template",
        "welcome_label",
        VARIABLES,
    )];
    const SECOND_DEFINITION: MailTemplateDefinition = MailTemplateDefinition::new(
        "password_reset",
        "mail_template",
        "password_reset_label",
        VARIABLES,
    );
    const DUPLICATE_CODE_DEFINITIONS: &[MailTemplateDefinition] = &[
        MailTemplateDefinition::new("welcome", "mail_template", "welcome_label", VARIABLES),
        MailTemplateDefinition::new("welcome", "mail_template", "welcome_label", VARIABLES),
    ];
    const DUPLICATE_VARIABLES: &[TemplateVariableSpec] = &[
        TemplateVariableSpec::new("username", "username_label", "username_desc"),
        TemplateVariableSpec::new("username", "username_label", "username_desc"),
    ];
    const DUPLICATE_VARIABLE_DEFINITIONS: &[MailTemplateDefinition] =
        &[MailTemplateDefinition::new(
            "welcome",
            "mail_template",
            "welcome_label",
            DUPLICATE_VARIABLES,
        )];

    #[test]
    fn registry_returns_variable_groups_in_definition_order() {
        let registry = MailTemplateRegistry::new(DEFINITIONS);

        let groups = registry.variable_groups();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].category, "mail_template");
        assert_eq!(groups[0].template_code, "welcome");
        assert_eq!(groups[0].label_i18n_key, "welcome_label");
        assert_eq!(groups[0].variables[0].token, "{{username}}");
        assert_eq!(
            registry.get("welcome").map(|definition| definition.code),
            Some("welcome")
        );
        assert!(registry.get("missing").is_none());
        registry.validate().unwrap();
    }

    #[test]
    fn catalog_builder_registers_multiple_sources_in_order() {
        let mut builder = MailTemplateCatalog::builder();
        builder.register_all(DEFINITIONS);
        builder.register(&SECOND_DEFINITION);
        let catalog = builder.build().unwrap();

        let codes = catalog
            .definitions()
            .iter()
            .map(|definition| definition.code)
            .collect::<Vec<_>>();

        assert_eq!(codes, vec!["welcome", "password_reset"]);
        assert_eq!(
            catalog
                .variable_groups()
                .into_iter()
                .map(|group| group.template_code)
                .collect::<Vec<_>>(),
            vec!["welcome", "password_reset"]
        );
        assert_eq!(
            catalog
                .get("password_reset")
                .map(|definition| definition.code),
            Some("password_reset")
        );
    }

    #[test]
    fn registry_validation_rejects_duplicate_template_codes() {
        let registry = MailTemplateRegistry::new(DUPLICATE_CODE_DEFINITIONS);

        assert_eq!(
            registry.validate(),
            Err(MailTemplateRegistryError::DuplicateTemplateCode { code: "welcome" })
        );
    }

    #[test]
    fn catalog_builder_rejects_duplicate_variable_keys() {
        let mut builder = MailTemplateCatalog::builder();
        builder.register_all(DUPLICATE_VARIABLE_DEFINITIONS);
        let error = builder.build().unwrap_err();

        assert_eq!(
            error,
            MailTemplateRegistryError::DuplicateVariableKey {
                template_code: "welcome",
                key: "username",
            }
        );
    }

    #[test]
    fn render_template_replaces_subject_html_and_text_placeholders() {
        let rendered = render_template(
            "Hello {{username}}".to_string(),
            "<p>Hello {{username}}</p><p>{{site_name}}</p>".to_string(),
            &TemplatePlaceholderSet::new(
                vec![
                    ("username", "A&B".to_string()),
                    ("site_name", "Aster".to_string()),
                ],
                vec![
                    ("username", escape_html("A&B")),
                    ("site_name", escape_html("Aster")),
                ],
            ),
        );

        assert_eq!(rendered.subject, "Hello A&B");
        assert_eq!(rendered.html_body, "<p>Hello A&amp;B</p><p>Aster</p>");
        assert_eq!(rendered.text_body, "Hello A&B\nAster");
    }

    #[test]
    fn html_to_text_ignores_head_script_and_style_content() {
        let html = "<!doctype html><html><head><title>Ignore</title><style>.x {}</style></head><body><p>Hello</p><script>bad()</script><ul><li>One</li></ul></body></html>";

        assert_eq!(html_to_text(html), "Hello\n- One");
    }
}

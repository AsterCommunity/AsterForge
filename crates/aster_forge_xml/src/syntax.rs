//! Shared XML syntax and parser error classification.

use crate::{Error, XmlSafetyError};

pub(crate) const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const XMLNS_NAMESPACE_URI: &str = "http://www.w3.org/2000/xmlns/";

pub(crate) fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| XmlSafetyError::InvalidEncoding.into())
}

pub(crate) fn validate_qualified_name(name: &str) -> Result<(Option<&str>, &str), Error> {
    let (prefix, local) = split_qualified_name(name);
    if !valid_name(local)
        || prefix.is_some_and(|prefix| !valid_name(prefix))
        || name.matches(':').count() > 1
    {
        return Err(XmlSafetyError::Malformed.into());
    }
    Ok((prefix, local))
}

pub(crate) fn split_qualified_name(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(is_name_start) && characters.all(is_name_char)
}

fn is_name_start(character: char) -> bool {
    matches!(
        character,
        'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{00C0}'..='\u{00D6}'
            | '\u{00D8}'..='\u{00F6}'
            | '\u{00F8}'..='\u{02FF}'
            | '\u{0370}'..='\u{037D}'
            | '\u{037F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || character.is_ascii_digit()
        || matches!(character, '-' | '.' | '\u{B7}')
        || ('\u{300}'..='\u{36F}').contains(&character)
        || ('\u{203F}'..='\u{2040}').contains(&character)
}

pub(crate) fn validate_namespace_binding(prefix: &str, uri: &str) -> Result<(), Error> {
    if prefix == "xmlns"
        || uri == XMLNS_NAMESPACE_URI
        || (prefix == "xml" && uri != XML_NAMESPACE_URI)
        || (prefix != "xml" && uri == XML_NAMESPACE_URI)
        || (!prefix.is_empty() && uri.is_empty())
    {
        Err(XmlSafetyError::Malformed.into())
    } else {
        Ok(())
    }
}

pub(crate) fn map_quick_xml_error(error: quick_xml::Error) -> Error {
    match error {
        quick_xml::Error::Encoding(_) => XmlSafetyError::InvalidEncoding.into(),
        error => Error::InvalidXml(error.to_string()),
    }
}

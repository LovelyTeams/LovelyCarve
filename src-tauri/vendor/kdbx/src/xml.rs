//! The decrypted XML as a small element tree. Protected values are freed
//! from the inner stream while parsing, in document order, as KeePass reads
//! them.

use base64::Engine;
use quick_xml::events::{BytesStart, Event};
use zeroize::Zeroizing;

use crate::crypto::InnerStream;
use crate::error::{Error, Result};
use crate::Limits;

/// The plaintext of a `Protected="True"` element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unprotected {
    Plain(Zeroizing<Vec<u8>>),
    /// The value could not be freed: not base64, or an inner stream this
    /// reader does not implement. The stored text stays in the element.
    Failed(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Element {
    pub(crate) name: String,
    pub(crate) attributes: Vec<(String, String)>,
    pub(crate) children: Vec<Node>,
    pub(crate) unprotected: Option<Unprotected>,
}

impl Element {
    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|element| element.name == name)
    }

    pub(crate) fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements().filter(move |element| element.name == name)
    }

    /// The element's text as stored.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        for node in &self.children {
            if let Node::Text(text) = node {
                out.push_str(text);
            }
        }
        out
    }

    pub(crate) fn is_protected(&self) -> bool {
        self.attribute("Protected") == Some("True")
    }
}

/// Parse decrypted XML, removing `stream` from protected values.
pub(crate) fn parse(xml: &[u8], stream: &mut InnerStream, limits: &Limits) -> Result<Element> {
    let text =
        std::str::from_utf8(xml).map_err(|error| Error::Xml(format!("not UTF-8: {error}")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = quick_xml::Reader::from_str(text);
    let xml_error = |reader: &quick_xml::Reader<&[u8]>, error: quick_xml::Error| {
        Error::Xml(format!("{error} at byte {}", reader.error_position()))
    };
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut count = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| xml_error(&reader, error))?;
        match event {
            Event::Start(start) => {
                count += 1;
                check(count, stack.len() + 1, limits)?;
                stack.push(open(&start)?);
            }
            Event::Empty(start) => {
                count += 1;
                check(count, stack.len() + 1, limits)?;
                let mut element = open(&start)?;
                finish(&mut element, stream)?;
                attach(&mut stack, &mut root, element)?;
            }
            Event::End(_) => {
                let mut element = stack
                    .pop()
                    .ok_or_else(|| Error::Xml("unbalanced end tag".into()))?;
                finish(&mut element, stream)?;
                attach(&mut stack, &mut root, element)?;
            }
            Event::Text(content) => push_text(&mut stack, &content.xml10_content()),
            Event::CData(content) => push_text(&mut stack, &content.xml10_content()),
            Event::GeneralRef(reference) => {
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(character)) => character.to_string(),
                    Ok(None) => quick_xml::escape::resolve_predefined_entity(&reference)
                        .ok_or_else(|| Error::Xml(format!("undefined entity &{};", &*reference)))?
                        .to_owned(),
                    Err(error) => return Err(xml_error(&reader, error)),
                };
                push_text(&mut stack, &resolved);
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if !stack.is_empty() {
        return Err(Error::Xml("unclosed elements at end of payload".into()));
    }
    root.ok_or_else(|| Error::Xml("no root element".into()))
}

fn check(count: usize, depth: usize, limits: &Limits) -> Result<()> {
    if count > limits.max_xml_elements {
        return Err(Error::Limit(format!(
            "more than {} XML elements",
            limits.max_xml_elements
        )));
    }
    if depth > limits.max_xml_depth {
        return Err(Error::Limit(format!(
            "XML nested deeper than {}",
            limits.max_xml_depth
        )));
    }
    Ok(())
}

fn open(start: &BytesStart<'_>) -> Result<Element> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| Error::Xml(error.to_string()))?;
        attributes.push((attribute.key.as_ref().to_owned(), value.into_owned()));
    }
    Ok(Element {
        name: start.name().as_ref().to_owned(),
        attributes,
        ..Element::default()
    })
}

fn push_text(stack: &mut [Element], text: &str) {
    // Text outside the root element is whitespace or rejected by the reader.
    let Some(element) = stack.last_mut() else {
        return;
    };
    match element.children.last_mut() {
        Some(Node::Text(existing)) => existing.push_str(text),
        _ => element.children.push(Node::Text(text.to_owned())),
    }
}

/// Close an element: drop the indentation between child elements and free a
/// protected value from the stream.
fn finish(element: &mut Element, stream: &mut InnerStream) -> Result<()> {
    if element.elements().next().is_some() {
        element
            .children
            .retain(|node| !matches!(node, Node::Text(text) if text.trim().is_empty()));
    }
    if element.is_protected() {
        element.unprotected = Some(unprotect(&element.text(), stream)?);
    }
    Ok(())
}

fn unprotect(text: &str, stream: &mut InnerStream) -> Result<Unprotected> {
    let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()) else {
        // KeePass refuses the file; the value is kept as stored and the
        // stream is not advanced, as pykeepass does.
        return Ok(Unprotected::Failed("protected value is not base64"));
    };
    let mut plain = Zeroizing::new(decoded);
    Ok(if stream.apply(&mut plain)? {
        Unprotected::Plain(plain)
    } else {
        Unprotected::Failed("inner stream algorithm not implemented")
    })
}

fn attach(stack: &mut [Element], root: &mut Option<Element>, element: Element) -> Result<()> {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Node::Element(element)),
        None if root.is_none() => *root = Some(element),
        None => return Err(Error::Xml("more than one root element".into())),
    }
    Ok(())
}

/// Write the tree back as XML, tab indented, with protected values in
/// plaintext (their `Protected="True"` attribute kept) as pykeepass does.
pub(crate) fn write(root: &Element) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n");
    write_element(root, 0, &mut out);
    out
}

fn write_element(element: &Element, depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push('\t');
    }
    out.push('<');
    out.push_str(&element.name);
    for (key, value) in &element.attributes {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(&quick_xml::escape::escape(value.as_str()));
        out.push('"');
    }
    let has_elements = element.elements().next().is_some();
    let text = match &element.unprotected {
        Some(Unprotected::Plain(plain)) => String::from_utf8_lossy(plain).into_owned(),
        Some(Unprotected::Failed(_)) | None => element.text(),
    };
    if !has_elements && text.is_empty() {
        out.push_str(" />\n");
        return;
    }
    out.push('>');
    if has_elements {
        out.push('\n');
        for child in element.elements() {
            write_element(child, depth + 1, out);
        }
        for _ in 0..depth {
            out.push('\t');
        }
    } else {
        out.push_str(&quick_xml::escape::escape(text.as_str()));
    }
    out.push_str("</");
    out.push_str(&element.name);
    out.push_str(">\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::InnerStreamAlgorithm;

    fn parse_plain(xml: &str) -> Element {
        parse(
            xml.as_bytes(),
            &mut InnerStream::new(InnerStreamAlgorithm::None, &[]),
            &Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn keeps_values_drops_indentation_and_resolves_references() {
        let root = parse_plain(
            "\u{feff}<?xml version=\"1.0\"?>\n<A>\n\t<B x=\"1 &amp; 2\">a &lt;b&gt; &#x4E2D;<![CDATA[<c>]]></B>\n\t<C>  </C>\n\t<D/>\n</A>",
        );
        assert_eq!(root.name, "A");
        assert_eq!(root.children.len(), 3);
        let b = root.child("B").unwrap();
        assert_eq!(b.attribute("x"), Some("1 & 2"));
        assert_eq!(b.text(), "a <b> 中<c>");
        assert_eq!(root.child("C").unwrap().text(), "  ");
        assert_eq!(root.child("D").unwrap().text(), "");
    }

    #[test]
    fn protected_values_consume_the_stream_in_document_order() {
        // With no inner stream, protected values are plain base64.
        let root = parse_plain(
            "<R><V Protected=\"True\">aGVsbG8=</V><V Protected=\"True\"></V><V>dGV4dA==</V><V Protected=\"True\">not base64!</V></R>",
        );
        let values: Vec<_> = root.children_named("V").collect();
        assert_eq!(
            values[0].unprotected,
            Some(Unprotected::Plain(Zeroizing::new(b"hello".to_vec())))
        );
        assert_eq!(
            values[1].unprotected,
            Some(Unprotected::Plain(Zeroizing::new(Vec::new())))
        );
        assert_eq!(values[2].unprotected, None);
        assert!(matches!(
            values[3].unprotected,
            Some(Unprotected::Failed(_))
        ));
        assert!(write(&root).contains("<V Protected=\"True\">hello</V>"));
    }

    #[test]
    fn rejects_malformed_and_oversized_trees() {
        let mut stream = InnerStream::new(InnerStreamAlgorithm::None, &[]);
        let limits = Limits::default();
        assert!(parse(b"<A><B></A>", &mut stream, &limits).is_err());
        assert!(parse(b"<A/><B/>", &mut stream, &limits).is_err());
        assert!(parse(b"<A>&bogus;</A>", &mut stream, &limits).is_err());
        let shallow = Limits {
            max_xml_depth: 2,
            ..Limits::default()
        };
        assert!(matches!(
            parse(b"<A><B><C/></B></A>", &mut stream, &shallow),
            Err(Error::Limit(_))
        ));
    }
}

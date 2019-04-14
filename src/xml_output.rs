use crate::model::xml::XmlElement;
use core::borrow::{Borrow, BorrowMut};
use log::trace;

use serde_json;
use std::io::Write;

use xml::common::XmlVersion;
use xml::{writer::XmlEvent, EmitterConfig, EventWriter};

use crate::binxml::name::BinXmlName;
use failure::{bail, format_err, Error};
use serde_json::{Map, Value};
use std::mem;

pub trait BinXMLOutput<'a, W: Write> {
    fn with_writer(target: W) -> Self;
    fn into_writer(self) -> Result<W, Error>;

    fn visit_end_of_stream(&mut self) -> Result<(), Error>;
    fn visit_open_start_element(
        &mut self,
        open_start_element: &XmlElement<'a>,
    ) -> Result<(), Error>;
    fn visit_close_element(&mut self) -> Result<(), Error>;
    fn visit_characters(&mut self, value: &str) -> Result<(), Error>;
    fn visit_cdata_section(&mut self) -> Result<(), Error>;
    fn visit_entity_reference(&mut self) -> Result<(), Error>;
    fn visit_processing_instruction_target(&mut self) -> Result<(), Error>;
    fn visit_processing_instruction_data(&mut self) -> Result<(), Error>;
    fn visit_start_of_stream(&mut self) -> Result<(), Error>;
}

pub struct XMLOutput<W: Write> {
    writer: EventWriter<W>,
    eof_reached: bool,
}

pub struct SerdeOutput<W: Write> {
    writer: W,
    map: Value,
    stack: Vec<String>,
    eof_reached: bool,
}

impl<W: Write> SerdeOutput<W> {
    /// Looks up the current path, will fill with empty objects if needed.
    fn get_or_create_current_path(&mut self) -> &mut Value {
        let mut v_temp = self.map.borrow_mut();

        for key in self.stack.iter() {
            // Current path does not exist yet, we need to create it.
            if v_temp.get(key).is_none() {
                // Can happen if we have
                // <Event>
                //    <System>
                //       <...>
                // since system has no attributes it has null and not an empty map.
                if v_temp.is_null() {
                    let mut map = Map::new();
                    map.insert(key.clone(), Value::Object(Map::new()));

                    mem::replace(v_temp, Value::Object(map));
                } else {
                    let current_object = v_temp
                        .as_object_mut()
                        .expect("It can only be an object or null, and null was covered");

                    current_object.insert(key.clone(), Value::Object(Map::new()));
                }
            }

            v_temp = v_temp.get_mut(key).expect("Loop above inserted this node.")
        }

        v_temp
    }

    fn get_current_parent(&mut self) -> &mut Value {
        // Make sure we are operating on created nodes.
        self.get_or_create_current_path();

        let mut v_temp = self.map.borrow_mut();

        for key in self.stack.iter().take(self.stack.len() - 1) {
            v_temp = v_temp
                .get_mut(key)
                .expect("Calling `get_or_create_current_path` ensures that the node was created")
        }

        v_temp
    }

    /// Like a regular node, but uses it's "Name" attribute.
    fn insert_data_node(&mut self, element: &XmlElement) -> Result<(), Error> {
        trace!("inserting data node");
        let name_attribute = element
            .attributes
            .iter()
            .find(|a| a.name == BinXmlName::from_static_string("Name"))
            .expect("Data node to have a name");

        let data_key: &str = name_attribute.value.borrow();
        self.insert_node_without_attributes(element, data_key)
    }

    fn insert_node_without_attributes(&mut self, _: &XmlElement, name: &str) -> Result<(), Error> {
        trace!("insert_node_without_attributes");
        self.stack.push(name.to_owned());

        let container = self.get_current_parent().as_object_mut().ok_or_else(|| {
            format_err!(
                "This is a bug - expected parent container to exist, and to be an object type.\
                 Check that the referenceing parent is not `Value::null`"
            )
        })?;

        container.insert(name.to_owned(), Value::Null);
        Ok(())
    }

    fn insert_node_with_attributes(
        &mut self,
        element: &XmlElement,
        name: &str,
    ) -> Result<(), Error> {
        trace!("insert_node_with_attributes");
        self.stack.push(name.to_owned());
        let value = self
            .get_or_create_current_path()
            .as_object_mut()
            .ok_or_else(|| {
                format_err!(
                    "This is a bug - expected current value to exist, and to be an object type.\
                     Check that the value is not `Value::null`"
                )
            })?;

        let mut attributes = Map::new();

        for attribute in element.attributes.iter() {
            let name: &str = attribute.name.borrow().into();
            let value_as_string: &str = attribute.value.borrow();

            attributes.insert(name.to_owned(), Value::String(value_as_string.to_owned()));
        }

        value.insert("#attributes".to_owned(), Value::Object(attributes));

        Ok(())
    }
}

impl<'a, W: Write> BinXMLOutput<'a, W> for SerdeOutput<W> {
    fn with_writer(target: W) -> Self {
        SerdeOutput {
            writer: target,
            map: Value::Object(Map::new()),
            stack: vec![],
            eof_reached: false,
        }
    }

    fn into_writer(mut self) -> Result<W, Error> {
        if self.eof_reached {
            if !self.stack.is_empty() {
                Err(format_err!(
                    "Invalid stream, EOF reached before closing all attributes"
                ))
            } else {
                serde_json::to_writer_pretty(&mut self.writer, &self.map)?;
                Ok(self.writer)
            }
        } else {
            Err(format_err!(
                "Tried to return writer before EOF marked, incomplete output."
            ))
        }
    }

    fn visit_end_of_stream(&mut self) -> Result<(), Error> {
        trace!("visit_end_of_stream");
        self.eof_reached = true;
        Ok(())
    }

    fn visit_open_start_element(&mut self, element: &XmlElement) -> Result<(), Error> {
        trace!("visit_open_start_element: {:?}", element.name);
        let element_name: &str = element.name.borrow().into();

        if element_name == "Data" {
            return self.insert_data_node(element);
        }

        // <Task>12288</Task> -> {"Task": 12288}
        if element.attributes.is_empty() {
            return self.insert_node_without_attributes(element, element_name);
        }

        self.insert_node_with_attributes(element, element_name)
    }

    fn visit_close_element(&mut self) -> Result<(), Error> {
        let p = self.stack.pop();
        trace!("visit_close_element: {:?}", p);
        Ok(())
    }

    fn visit_characters(&mut self, value: &str) -> Result<(), Error> {
        trace!("visit_chars {:?}", &self.stack);
        let current_value = self.get_or_create_current_path();

        // If our parent is an element without any attributes,
        // we simply swap the null with the string value.
        if current_value.is_null() {
            mem::replace(current_value, Value::String(value.to_owned()));
        } else {
            // Should look like:
            // ----------------
            //  "EventID": {
            //    "#attributes": {
            //      "Qualifiers": ""
            //    },
            //    "#text": "4902"
            //  },
            let current_object = current_value.as_object_mut().ok_or_else(|| {
                format_err!("This is a bug - expected current value to be an object type")
            })?;

            current_object.insert("#text".to_owned(), Value::String(value.to_owned()));
        }

        Ok(())
    }

    fn visit_cdata_section(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn visit_entity_reference(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn visit_processing_instruction_target(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn visit_processing_instruction_data(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn visit_start_of_stream(&mut self) -> Result<(), Error> {
        trace!("visit_start_of_stream");
        Ok(())
    }
}

/// Adapter between binxml XmlModel type and rust-xml output stream.
impl<'a, W: Write> BinXMLOutput<'a, W> for XMLOutput<W> {
    fn with_writer(target: W) -> Self {
        let writer = EmitterConfig::new()
            .line_separator("\r\n")
            .perform_indent(true)
            .normalize_empty_elements(false)
            .create_writer(target);

        XMLOutput {
            writer,
            eof_reached: false,
        }
    }

    fn into_writer(self) -> Result<W, Error> {
        if self.eof_reached {
            Ok(self.writer.into_inner())
        } else {
            Err(format_err!(
                "Tried to return writer before EOF marked, incomplete output."
            ))
        }
    }

    fn visit_end_of_stream(&mut self) -> Result<(), Error> {
        trace!("visit_end_of_stream");
        self.eof_reached = true;
        Ok(())
    }

    fn visit_open_start_element(&mut self, element: &XmlElement) -> Result<(), Error> {
        trace!("visit_open_start_element: {:?}", element);
        if self.eof_reached {
            bail!("Impossible state - `visit_open_start_element` after EOF");
        }

        let mut event_builder = XmlEvent::start_element(element.name.borrow());

        for attr in element.attributes.iter() {
            event_builder = event_builder.attr(attr.name.borrow(), &attr.value.borrow());
        }

        self.writer.write(event_builder)?;

        Ok(())
    }

    fn visit_close_element(&mut self) -> Result<(), Error> {
        trace!("visit_close_element");
        self.writer.write(XmlEvent::end_element())?;
        Ok(())
    }

    fn visit_characters(&mut self, value: &str) -> Result<(), Error> {
        trace!("visit_chars");
        self.writer.write(XmlEvent::characters(value))?;
        Ok(())
    }

    fn visit_cdata_section(&mut self) -> Result<(), Error> {
        unimplemented!("visit_cdata_section");
    }

    fn visit_entity_reference(&mut self) -> Result<(), Error> {
        unimplemented!("visit_entity_reference");
    }

    fn visit_processing_instruction_target(&mut self) -> Result<(), Error> {
        unimplemented!("visit_processing_instruction_target");
    }

    fn visit_processing_instruction_data(&mut self) -> Result<(), Error> {
        unimplemented!("visit_processing_instruction_data");
    }

    fn visit_start_of_stream(&mut self) -> Result<(), Error> {
        trace!("visit_start_of_stream");
        if self.eof_reached {
            bail!("Impossible state - `visit_start_of_stream` after EOF");
        }

        self.writer.write(XmlEvent::StartDocument {
            version: XmlVersion::Version10,
            encoding: None,
            standalone: None,
        })?;

        Ok(())
    }
}

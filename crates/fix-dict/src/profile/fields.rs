use super::raw::{RawField, Ref};
use super::{FIELDS_MAX, Loader, ProfileErrorKind, VALUES_MAX};
use crate::schema::FieldType;
use crate::value::matches_type;
use crate::venue::CustomField;
use toml::Spanned;

impl Loader<'_> {
    /// Adds the `[[field]]` entries: new fields, and more values for
    /// standard fields.
    pub(super) fn load_fields(&mut self) {
        let raw = self.raw;
        let mut seen: Vec<u32> = Vec::new();
        for entry in &raw.field {
            let tag = *entry.get_ref().tag.get_ref();
            let start = entry.get_ref().tag.span().start;
            if seen.contains(&tag) {
                self.error(start, ProfileErrorKind::DuplicateField(tag));
                continue;
            }
            seen.push(tag);
            if self.venue.fields.len() >= FIELDS_MAX {
                self.error(start, ProfileErrorKind::TooMany("[[field]] entries"));
                return;
            }
            if self.venue.standard.field(tag).is_some() {
                self.extend_standard_field(entry.get_ref(), tag);
            } else {
                self.add_new_field(entry.get_ref(), tag);
            }
        }
        self.venue.fields.sort_by_key(|field| field.tag);
    }

    fn parse_type(&mut self, ty: &Spanned<String>) -> Option<FieldType> {
        let parsed = FieldType::from_name(ty.get_ref());
        if parsed.is_none() {
            let kind = ProfileErrorKind::UnknownType(ty.get_ref().clone());
            self.error(ty.span().start, kind);
        }
        parsed
    }

    /// Checks the values of a field and returns them. A value must have the
    /// syntax of the field type.
    fn field_values(&mut self, entry: &RawField, tag: u32, ty: FieldType) -> Vec<String> {
        if entry.values.len() > VALUES_MAX {
            self.error(entry.tag.span().start, ProfileErrorKind::TooMany("values"));
            return Vec::new();
        }
        let mut values = Vec::new();
        for value in &entry.values {
            let text = value.get_ref();
            // Each value of a MultipleValueString field is one word.
            let all_ok = if ty == FieldType::MultipleValueString {
                !text.contains(' ')
            } else {
                matches_type(ty, text.as_bytes())
            };
            if all_ok && !text.is_empty() {
                values.push(text.clone());
            } else {
                let kind = ProfileErrorKind::ValueNotOfType {
                    tag,
                    value: text.clone(),
                    ty,
                };
                self.error(value.span().start, kind);
            }
        }
        values
    }

    fn extend_standard_field(&mut self, entry: &RawField, tag: u32) {
        let Some(def) = self.venue.standard.field(tag) else {
            return;
        };
        if let Some(name) = &entry.name
            && name.get_ref() != def.name
        {
            let kind = ProfileErrorKind::FieldConflict {
                tag,
                name: name.get_ref().clone(),
            };
            self.error(name.span().start, kind);
        }
        if let Some(ty) = &entry.ty
            && self.parse_type(ty).is_some_and(|parsed| parsed != def.ty)
        {
            let kind = ProfileErrorKind::FieldConflict {
                tag,
                name: def.name.to_owned(),
            };
            self.error(ty.span().start, kind);
        }
        let values = self.field_values(entry, tag, def.ty);
        if !values.is_empty() {
            self.venue.extra_values.push((tag, values));
        }
    }

    fn add_new_field(&mut self, entry: &RawField, tag: u32) {
        let start = entry.tag.span().start;
        let Some(name) = &entry.name else {
            self.error(start, ProfileErrorKind::MissingKey("name"));
            return;
        };
        let Some(ty) = &entry.ty else {
            self.error(start, ProfileErrorKind::MissingKey("type"));
            return;
        };
        let Some(parsed) = self.parse_type(ty) else {
            return;
        };
        let taken = self.venue.field_by_name(name.get_ref()).is_some();
        if taken || name.get_ref().is_empty() {
            let kind = ProfileErrorKind::FieldConflict {
                tag,
                name: name.get_ref().clone(),
            };
            self.error(name.span().start, kind);
            return;
        }
        let values = self.field_values(entry, tag, parsed);
        self.venue.fields.push(CustomField {
            tag,
            name: name.get_ref().clone(),
            ty: parsed,
            values,
        });
    }

    /// Adds the `[defaults]`. The key is a field name or a tag.
    pub(super) fn load_defaults(&mut self) {
        let raw = self.raw;
        for (key, value) in &raw.defaults {
            let reference = key
                .parse::<u32>()
                .map_or_else(|_| Ref::Name(key.clone()), Ref::Tag);
            let start = value.span().start;
            let tag = match self.field_tag(&reference) {
                Ok(tag) => tag,
                Err(kind) => {
                    self.error(start, kind);
                    continue;
                }
            };
            self.check_default(tag, value.get_ref(), start);
            self.venue.defaults.push((tag, value.get_ref().clone()));
        }
        self.venue.defaults.sort_by_key(|(tag, _)| *tag);
    }

    fn check_default(&mut self, tag: u32, value: &str, start: usize) {
        let Some(info) = self.venue.field(tag) else {
            return;
        };
        let (ty, has_values, accepted) = (info.ty, info.has_values(), info.accepts_value(value));
        if has_values && !accepted {
            let kind = ProfileErrorKind::EnumDefaultInvalid {
                tag,
                value: value.to_owned(),
            };
            self.error(start, kind);
        } else if !has_values && !matches_type(ty, value.as_bytes()) {
            let kind = ProfileErrorKind::ValueNotOfType {
                tag,
                value: value.to_owned(),
                ty,
            };
            self.error(start, kind);
        }
    }
}

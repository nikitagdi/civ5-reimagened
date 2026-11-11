//! `Schema` as the package and the expression checker see it (`ui2_package::profile::SchemaView`).

use ui2_package::profile::{SchemaView, TypeSig};

use crate::schema::{FieldDef, Scalar, Schema, StructDef, Type};

impl Schema {
    /// The type of a schema type for the package: integers are `Int`, a record is `Struct` by
    /// its type name, a list is `List` of its element's record.
    fn type_sig(&self, ty: &Type) -> TypeSig {
        match ty {
            Type::Scalar(s) => match s {
                Scalar::Bool => TypeSig::Bool,
                Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::U32 => TypeSig::Int,
                Scalar::F32 => TypeSig::Float,
                Scalar::Str => TypeSig::Str,
                Scalar::Text => TypeSig::Text,
                Scalar::Asset => TypeSig::Asset,
                Scalar::Color => TypeSig::Color,
            },
            Type::Key(kind) => TypeSig::Key(kind.clone()),
            Type::Enum(e) => TypeSig::Enum(self.enum_def(*e).variants.clone()),
            Type::Struct(t) => TypeSig::Struct(self.type_def(*t).name.clone()),
            Type::Option(inner) => TypeSig::Opt(Box::new(self.type_sig(inner))),
            Type::List(l) => TypeSig::List(Box::new(TypeSig::Struct(
                self.type_def(l.elem).name.clone(),
            ))),
        }
    }

    fn struct_named(&self, name: &str) -> Option<&StructDef> {
        self.types().iter().find(|t| t.name == name)
    }

    fn field_def(&self, of: &TypeSig, name: &str) -> Option<&FieldDef> {
        let of = match of {
            TypeSig::Opt(inner) => inner.as_ref(),
            other => other,
        };
        let TypeSig::Struct(type_name) = of else {
            return None;
        };
        self.struct_named(type_name)?
            .fields
            .iter()
            .find(|f| f.name == name)
    }
}

impl SchemaView for Schema {
    fn root(&self, name: &str) -> Option<TypeSig> {
        // The whole root: its record, an optional record or a list.
        Some(self.type_sig(&self.root_type(self.root_by_name(name)?)))
    }

    fn field(&self, of: &TypeSig, name: &str) -> Option<TypeSig> {
        let field = self.field_def(of, name)?;
        Some(self.type_sig(&field.ty))
    }

    fn list_key(&self, list: &TypeSig) -> Option<&str> {
        let TypeSig::List(elem) = list else {
            return None;
        };
        let TypeSig::Struct(name) = elem.as_ref() else {
            return None;
        };
        let def = self.struct_named(name)?;
        Some(def.fields.get(def.key_index?)?.name.as_str())
    }

    fn intent(&self, name: &str) -> Option<Vec<(String, TypeSig)>> {
        let intent = self.intents().iter().find(|i| i.name == name)?;
        Some(
            intent
                .args
                .iter()
                .map(|a| (a.name.clone(), self.type_sig(&a.ty)))
                .collect(),
        )
    }

    fn js_writable(&self, path: &str) -> bool {
        self.resolve(path).is_ok_and(|field| self.path(field.id).js)
    }

    fn version(&self) -> (u16, u16) {
        let v = Schema::version(self);
        (v.major, v.minor)
    }
}

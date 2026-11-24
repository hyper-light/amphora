//! Utility statements as the engine runs them: CREATE TABLE of plain columns (`transformCreateStmt`,
//! REL_19_0 src/backend/parser/parse_utilcmd.c; `DefineRelation`, src/backend/commands/tablecmds.c) and
//! ALTER TABLE ADD PRIMARY KEY (`transformIndexConstraint`, parse_utilcmd.c). Every other clause of
//! them refuses `0A000`.

use amphora_pg_catalog::data::type_oid;
use amphora_pg_catalog::{Kind, TypeError, column_type};
use amphora_pg_parser::tree::{ColumnDef, Constraint, CreateStmt, Node, NodeId, Tree, Val};

use crate::lower::quoted;
use crate::{Error, Session, sqlstate};

/// parsenodes.h's CONSTR_NOTNULL and CONSTR_PRIMARY, AT_AddConstraint and OBJECT_TABLE, as the grammar
/// numbers them.
const NOT_NULL: i32 = 1;
const PRIMARY: i32 = 6;
const ADD_CONSTRAINT: i32 = 16;
const TABLE: i32 = 42;

/// The longest name, in bytes: `NAMEDATALEN - 1` (src/include/pg_config_manual.h:39).
const NAME_MAX: usize = 63;

/// The bounds of a heap's `fillfactor` (src/backend/access/common/reloptions.c:175,
/// `HEAP_MIN_FILLFACTOR` and 100). It places rows on pages, and no statement's answer turns on it.
const FILLFACTOR: std::ops::RangeInclusive<i32> = 10..=100;

/// A utility statement as the engine runs it, and its command tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utility {
    pub sql: String,
    pub tag: &'static str,
}

/// The engine's statement of a raw utility statement: `None` for a statement [`crate::analyze`] types.
pub fn utility(
    tree: &Tree,
    statement: NodeId,
    session: &Session<'_>,
) -> Result<Option<Utility>, Error> {
    let Node::RawStmt {
        stmt: Some(stmt), ..
    } = tree.get(statement)?
    else {
        return Ok(None);
    };
    match tree.get(*stmt)? {
        Node::CreateStmt(create) => create_table(tree, create, session).map(Some),
        Node::AlterTableStmt {
            relation,
            cmds,
            objtype: TABLE,
            missing_ok: false,
        } => add_primary_key(tree, *relation, cmds, session).map(Some),
        _ => Ok(None),
    }
}

/// The name a RangeVar gives a table, in no schema: the parser's ported rules read no other.
fn table_name(tree: &Tree, relation: Option<NodeId>) -> Result<&str, Error> {
    match relation.map(|id| tree.get(id)).transpose()? {
        Some(Node::RangeVar {
            catalogname: None,
            schemaname: None,
            relname,
            inh: true,
            relpersistence: 'p',
            alias: None,
            ..
        }) => Ok(relname),
        _ => Err(Error::unsupported("this table's name")),
    }
}

fn create_table(tree: &Tree, s: &CreateStmt, session: &Session<'_>) -> Result<Utility, Error> {
    let plain = CreateStmt {
        relation: s.relation,
        table_elts: s.table_elts.clone(),
        options: s.options.clone(),
        ..CreateStmt::default()
    };
    if plain != *s {
        return Err(Error::unsupported("a clause of this CREATE TABLE"));
    }
    for option in &s.options {
        storage_parameter(tree, *option)?;
    }
    let table = table_name(tree, s.relation)?;
    // RangeVarGetCreationNamespace: the path's first schema but pg_catalog, which is `public` where no
    // schema can be made (section 7).
    let public = session
        .relations
        .namespace("public")
        .ok_or_else(|| Error::unsupported("a database with no public schema"))?;
    let mut columns: Vec<(&str, String)> = Vec::with_capacity(s.table_elts.len());
    for element in &s.table_elts {
        let Node::ColumnDef(column) = tree.get(*element)? else {
            return Err(Error::unsupported("a table constraint"));
        };
        columns.push(self::column(tree, column)?);
    }
    if columns.is_empty() {
        return Err(Error::unsupported("a table of no columns"));
    }
    for (at, (name, _)) in columns.iter().enumerate() {
        if columns.iter().take(at).any(|(earlier, _)| earlier == name) {
            return Err(Error::new(
                sqlstate::DUPLICATE_COLUMN,
                format!("column \"{name}\" specified more than once"),
            ));
        }
    }
    if session.relations.relation(public, table).is_some() {
        return Err(Error::new(
            sqlstate::DUPLICATE_TABLE,
            format!("relation \"{table}\" already exists"),
        ));
    }
    let definitions: Vec<&str> = columns.iter().map(|(_, text)| text.as_str()).collect();
    Ok(Utility {
        sql: format!("CREATE TABLE {}({})", quoted(table), definitions.join(", ")),
        tag: "CREATE TABLE",
    })
}

/// A column's name, and its definition in the engine's text.
fn column<'a>(tree: &'a Tree, c: &'a ColumnDef) -> Result<(&'a str, String), Error> {
    let plain = ColumnDef {
        colname: c.colname.clone(),
        type_name: c.type_name,
        is_local: true,
        constraints: c.constraints.clone(),
        location: c.location,
        ..ColumnDef::default()
    };
    let (Some(name), Some(type_name)) = (&c.colname, c.type_name) else {
        return Err(Error::unsupported("this column").at(c.location));
    };
    if plain != *c {
        return Err(Error::unsupported("a clause of this column").at(c.location));
    }
    let mut text = format!("{} {}", quoted(name), type_text(tree, type_name)?);
    for id in &c.constraints {
        let not_null = match tree.get(*id)? {
            Node::Constraint(k) => {
                **k == Constraint {
                    contype: NOT_NULL,
                    is_enforced: true,
                    initially_valid: true,
                    location: k.location,
                    ..Constraint::default()
                }
            }
            _ => false,
        };
        if !not_null {
            return Err(Error::unsupported("this column constraint").at(c.location));
        }
        text.push_str(" NOT NULL");
    }
    Ok((name, text))
}

/// A column type's name in `pg_catalog` with its modifiers, which the engine's dialect reads as 19's
/// type of that name: one of the unit's types, which section 5 stores.
fn type_text(tree: &Tree, id: NodeId) -> Result<String, Error> {
    let Node::TypeName {
        names,
        type_oid: 0,
        setof: false,
        pct_type: false,
        typmods,
        typemod: -1,
        array_bounds,
        location,
    } = tree.get(id)?
    else {
        return Err(Error::unsupported("this type"));
    };
    let mut words = Vec::with_capacity(names.len());
    for name in names {
        match tree.get(*name)? {
            Node::String(word) => words.push(word.as_str()),
            _ => return Err(Error::unsupported("this type").at(*location)),
        }
    }
    let name = match words.as_slice() {
        ["pg_catalog", name] | [name] if array_bounds.is_empty() => *name,
        _ => return Err(Error::unsupported("this type").at(*location)),
    };
    // transformColumnDefinition: a type of no row in pg_type, its column an integer with a sequence.
    if matches!(
        name,
        "smallserial" | "serial2" | "serial" | "serial4" | "bigserial" | "serial8"
    ) {
        return Err(Error::unsupported(&format!("a column of type {name}")).at(*location));
    }
    let mut modifiers = Vec::with_capacity(typmods.len());
    for typmod in typmods {
        match tree.get(*typmod)? {
            Node::AConst {
                val: Val::Integer(n),
                ..
            } => modifiers.push(i64::from(*n)),
            _ => return Err(Error::unsupported("this type modifier").at(*location)),
        }
    }
    let found = column_type(&format!("\"{name}\""), &modifiers).map_err(|refused| {
        let (code, message) = match refused {
            TypeError::Undefined(message) => (sqlstate::UNDEFINED_OBJECT, message),
            TypeError::Modifier(message) => (sqlstate::INVALID_PARAMETER_VALUE, message),
        };
        Error::new(code, message).at(*location)
    })?;
    match (found.oid, modifiers.as_slice()) {
        (
            type_oid::INT4
            | type_oid::INT8
            | type_oid::TEXT
            | type_oid::BPCHAR
            | type_oid::TIMESTAMP
            | type_oid::TIMESTAMPTZ,
            [],
        ) => Ok(name.to_owned()),
        (type_oid::BPCHAR, [width]) => Ok(format!("{name}({width})")),
        _ => Err(Error::unsupported(&format!(
            "a column of type {}",
            crate::format_type(found.oid)
        ))
        .at(*location)),
    }
}

/// A heap's storage parameter, of which only `fillfactor` is taken.
fn storage_parameter(tree: &Tree, id: NodeId) -> Result<(), Error> {
    if let Node::DefElem {
        defnamespace: None,
        defname,
        arg: Some(arg),
        defaction: 0,
        ..
    } = tree.get(id)?
        && defname == "fillfactor"
        && let Node::Integer(percent) = tree.get(*arg)?
        && FILLFACTOR.contains(percent)
    {
        return Ok(());
    }
    Err(Error::unsupported("this storage parameter"))
}

fn add_primary_key(
    tree: &Tree,
    relation: Option<NodeId>,
    cmds: &[NodeId],
    session: &Session<'_>,
) -> Result<Utility, Error> {
    let [cmd] = cmds else {
        return Err(Error::unsupported("an ALTER TABLE of several commands"));
    };
    let Node::AlterTableCmd {
        subtype: ADD_CONSTRAINT,
        name: None,
        num: 0,
        newowner: None,
        def: Some(def),
        behavior: 0,
        missing_ok: false,
        ..
    } = tree.get(*cmd)?
    else {
        return Err(Error::unsupported("this ALTER TABLE"));
    };
    let Node::Constraint(constraint) = tree.get(*def)? else {
        return Err(Error::unsupported("this ALTER TABLE"));
    };
    let plain = Constraint {
        contype: PRIMARY,
        keys: constraint.keys.clone(),
        location: constraint.location,
        ..Constraint::default()
    };
    if plain != **constraint {
        return Err(Error::unsupported("this constraint").at(constraint.location));
    }
    let table = table_name(tree, relation)?;
    let found = session
        .path
        .resolve(Kind::Relation, |ns| session.relations.relation(ns, table));
    // RangeVarGetRelidExtended refuses with no position (src/backend/catalog/namespace.c).
    let Some(oid) = found else {
        return Err(Error::new(
            sqlstate::UNDEFINED_TABLE,
            format!("relation \"{table}\" does not exist"),
        ));
    };
    // The parser's ported rules read one column of a key.
    let [key] = constraint.keys.as_slice() else {
        return Err(Error::unsupported("a primary key of several columns"));
    };
    let Node::String(key) = tree.get(*key)? else {
        return Err(Error::unsupported("this key"));
    };
    let columns = session.relations.columns(oid);
    // ATPrepAddPrimaryKey, before the key's own checks (src/backend/commands/tablecmds.c).
    let Some(column) = columns.iter().find(|c| c.name == *key) else {
        return Err(Error::new(
            sqlstate::UNDEFINED_COLUMN,
            format!("column \"{key}\" of relation \"{table}\" does not exist"),
        ));
    };
    if !column.not_null {
        return Err(Error::unsupported(
            "a primary key on a column without NOT NULL",
        ));
    }
    // ChooseIndexName: `<table>_pkey`, which 19 shortens or numbers where it doesn't fit or is taken.
    let index = format!("{table}_pkey");
    if index.len() > NAME_MAX {
        return Err(Error::unsupported("a primary key's name this long"));
    }
    Ok(Utility {
        sql: format!(
            "CREATE UNIQUE INDEX {} ON {}({})",
            quoted(&index),
            quoted(table),
            quoted(key)
        ),
        tag: "ALTER TABLE",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, Relations};
    use amphora_pg_catalog::SearchPath;
    use amphora_pg_parser::gram;

    struct Public;

    impl Relations for Public {
        fn namespace(&self, name: &str) -> Option<u32> {
            (name == "public").then(|| 2200)
        }

        fn relation(&self, namespace: u32, name: &str) -> Option<u32> {
            (namespace == 2200 && name == "t").then(|| 16384)
        }

        fn name(&self, oid: u32) -> Option<(u32, String)> {
            (oid == 16384).then(|| (2200, "t".to_owned()))
        }

        fn namespace_name(&self, oid: u32) -> Option<String> {
            (oid == 2200).then(|| "public".to_owned())
        }

        fn columns(&self, _oid: u32) -> Vec<Column> {
            [("aid", true), ("bid", false)]
                .map(|(name, not_null)| Column {
                    name: name.to_owned(),
                    type_oid: type_oid::INT4,
                    typmod: -1,
                    not_null,
                })
                .to_vec()
        }
    }

    fn lowered(sql: &str) -> Result<Option<Utility>, Error> {
        let zone = crate::tz::time_zone("UTC").unwrap();
        let path = SearchPath::new(&["public"], "postgres", None, |name| Public.namespace(name));
        let session = Session {
            path: &path,
            relations: &Public,
            time_zone: &zone,
            now: None,
        };
        let (tree, statements) = gram::parse(sql.as_bytes()).expect(sql);
        utility(&tree, statements[0], &session)
    }

    /// T-PG.9.3: pgbench's table and its primary key lower to the engine's, each type by its name in
    /// `pg_type`, and a statement analysis types is not a utility statement.
    #[test]
    fn t_pg_9_3_pgbench_tables_and_keys_lower_to_the_engines() {
        assert_eq!(
            lowered("create table pgbench_tellers(tid int not null,bid int,mtime timestamp,filler char(84)) with (fillfactor=100)").unwrap(),
            Some(Utility {
                sql: "CREATE TABLE \"pgbench_tellers\"(\"tid\" int4 NOT NULL, \"bid\" int4, \"mtime\" timestamp, \"filler\" bpchar(84))".to_owned(),
                tag: "CREATE TABLE",
            })
        );
        assert_eq!(
            lowered("alter table t add primary key (aid)").unwrap(),
            Some(Utility {
                sql: "CREATE UNIQUE INDEX \"t_pkey\" ON \"t\"(\"aid\")".to_owned(),
                tag: "ALTER TABLE",
            })
        );
        assert_eq!(lowered("SELECT 1").unwrap(), None);
    }

    /// T-PG.9.3: what isn't built refuses `0A000`, and what 19 refuses carries its SQLSTATE.
    #[test]
    fn t_pg_9_3_other_tables_and_keys_are_refused() {
        for (sql, sqlstate) in [
            ("create table t(a int)", sqlstate::DUPLICATE_TABLE),
            ("create table u(a int, a int)", sqlstate::DUPLICATE_COLUMN),
            ("create table u(a nosuch)", sqlstate::UNDEFINED_OBJECT),
            (
                "create table u(a char(0))",
                sqlstate::INVALID_PARAMETER_VALUE,
            ),
            ("create table u(a serial)", sqlstate::FEATURE_NOT_SUPPORTED),
            ("create table u(a uuid)", sqlstate::FEATURE_NOT_SUPPORTED),
            (
                "create table u(a int) with (fillfactor=5)",
                sqlstate::FEATURE_NOT_SUPPORTED,
            ),
            (
                "create table u(a int) with (autovacuum_enabled=1)",
                sqlstate::FEATURE_NOT_SUPPORTED,
            ),
            (
                "alter table u add primary key (aid)",
                sqlstate::UNDEFINED_TABLE,
            ),
            (
                "alter table t add primary key (nope)",
                sqlstate::UNDEFINED_COLUMN,
            ),
            (
                "alter table t add primary key (bid)",
                sqlstate::FEATURE_NOT_SUPPORTED,
            ),
        ] {
            let refused = lowered(sql).map_err(|e| e.sqlstate);
            assert_eq!(refused, Err(sqlstate), "{sql}");
        }
    }
}

//! The permission tree: what the software *has*, as opposed to what anyone
//! holds.
//!
//! Declared in code, identical for every tenant, compiled into both the server
//! and the browser bundle. Adding a permission is a code change, because the
//! code is what enforces it - a permission nothing checks is decoration.
//!
//! ```text
//! Pages
//!  +- Pages.Dashboard
//!  +- Pages.Files
//!  |   +- .Upload  .Delete
//!  +- Pages.Sales
//!  |   +- Pages.Sales.Invoices
//!  |       +- .Create  .Edit  .Post  .Void
//!  +- Pages.Master
//!  |   +- Pages.Master.Parties
//!  |   |   +- .Create  .Edit  .Delete
//!  |   +- Pages.Master.Taxes
//!  |       +- .Edit
//!  +- Pages.Administration
//!      +- Pages.Administration.Users
//!      |   +- .Create  .Edit  .Delete  .ChangePermissions  .Impersonate
//!      +- Pages.Administration.Roles
//!      |   +- .Create  .Edit  .Delete  .ChangePermissions
//!      +- Pages.Administration.Settings
//!      +- Pages.Administration.AuditLogs
//!      +- Pages.Administration.Apps
//!          +- .Install
//! ```

use serde::{Deserialize, Serialize};

/// Every permission name, as a constant.
///
/// Always refer to a permission through one of these rather than by writing the
/// string at the call site: a typo in a literal fails *open* - the check simply
/// never matches a granted name, and the guard silently does nothing.
pub mod names {
    pub const PAGES: &str = "Pages";
    pub const DASHBOARD: &str = "Pages.Dashboard";

    pub const FILES: &str = "Pages.Files";
    pub const FILES_UPLOAD: &str = "Pages.Files.Upload";
    pub const FILES_DELETE: &str = "Pages.Files.Delete";

    pub const SALES: &str = "Pages.Sales";

    pub const INVOICES: &str = "Pages.Sales.Invoices";
    pub const INVOICES_CREATE: &str = "Pages.Sales.Invoices.Create";
    pub const INVOICES_EDIT: &str = "Pages.Sales.Invoices.Edit";
    pub const INVOICES_POST: &str = "Pages.Sales.Invoices.Post";
    pub const INVOICES_VOID: &str = "Pages.Sales.Invoices.Void";

    pub const MASTER: &str = "Pages.Master";

    pub const PARTIES: &str = "Pages.Master.Parties";
    pub const PARTIES_CREATE: &str = "Pages.Master.Parties.Create";
    pub const PARTIES_EDIT: &str = "Pages.Master.Parties.Edit";
    pub const PARTIES_DELETE: &str = "Pages.Master.Parties.Delete";

    pub const TAXES: &str = "Pages.Master.Taxes";
    pub const TAXES_EDIT: &str = "Pages.Master.Taxes.Edit";

    pub const ADMINISTRATION: &str = "Pages.Administration";

    pub const USERS: &str = "Pages.Administration.Users";
    pub const USERS_CREATE: &str = "Pages.Administration.Users.Create";
    pub const USERS_EDIT: &str = "Pages.Administration.Users.Edit";
    pub const USERS_DELETE: &str = "Pages.Administration.Users.Delete";
    pub const USERS_CHANGE_PERMISSIONS: &str = "Pages.Administration.Users.ChangePermissions";
    pub const USERS_IMPERSONATE: &str = "Pages.Administration.Users.Impersonate";

    pub const ROLES: &str = "Pages.Administration.Roles";
    pub const ROLES_CREATE: &str = "Pages.Administration.Roles.Create";
    pub const ROLES_EDIT: &str = "Pages.Administration.Roles.Edit";
    pub const ROLES_DELETE: &str = "Pages.Administration.Roles.Delete";
    pub const ROLES_CHANGE_PERMISSIONS: &str = "Pages.Administration.Roles.ChangePermissions";

    pub const SETTINGS: &str = "Pages.Administration.Settings";
    pub const AUDIT_LOGS: &str = "Pages.Administration.AuditLogs";

    pub const APPS: &str = "Pages.Administration.Apps";
    pub const APPS_INSTALL: &str = "Pages.Administration.Apps.Install";
}

/// One node of the permission tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDefinition {
    /// Dotted, stable, and stored verbatim in `role_permissions.name`.
    ///
    /// Renaming one is a data migration, not an edit - existing grants are
    /// keyed by this string.
    pub name: &'static str,
    /// What the role editor shows.
    pub display_name: &'static str,
    pub description: Option<&'static str>,
    /// The dotted prefix one level up. `None` only for a root.
    pub parent: Option<&'static str>,
    /// Granted to the static `User` role in every new workspace.
    pub default_for_user: bool,
}

impl PermissionDefinition {
    /// The last dotted segment, e.g. `Create`.
    pub fn leaf(&self) -> &'static str {
        self.name.rsplit('.').next().unwrap_or(self.name)
    }

    /// How deep in the tree, with a root at 0. Drives indentation in the role
    /// editor.
    pub fn depth(&self) -> usize {
        self.name.matches('.').count()
    }
}

/// The complete tree, in depth-first order.
///
/// Declaration order is the display order in the role editor, so parents come
/// before their children and siblings are grouped. A test enforces both.
pub const DEFINITIONS: &[PermissionDefinition] = &[
    PermissionDefinition {
        name: names::PAGES,
        display_name: "Pages",
        description: Some("Access the application at all."),
        parent: None,
        // Without this the User role cannot reach any page, since every other
        // permission hangs beneath it.
        default_for_user: true,
    },
    PermissionDefinition {
        name: names::DASHBOARD,
        display_name: "Dashboard",
        description: Some("View the workspace dashboard."),
        parent: Some(names::PAGES),
        default_for_user: true,
    },
    // -- Files ------------------------------------------------------------
    //
    // Not under Administration: uploading an attachment is ordinary work, and
    // putting it there would mean granting the administration area to anybody
    // who needs to attach a document. Deleting is the exception - a stored file
    // is a record, and removing one is not the same act as adding one.
    PermissionDefinition {
        name: names::FILES,
        display_name: "Files",
        description: Some("See the files stored in this workspace."),
        parent: Some(names::PAGES),
        default_for_user: true,
    },
    PermissionDefinition {
        name: names::FILES_UPLOAD,
        display_name: "Upload",
        description: Some("Add files to this workspace."),
        parent: Some(names::FILES),
        default_for_user: true,
    },
    PermissionDefinition {
        name: names::FILES_DELETE,
        display_name: "Delete",
        description: Some("Remove a stored file."),
        parent: Some(names::FILES),
        default_for_user: false,
    },
    // -- Sales ------------------------------------------------------------
    //
    // Four powers, because they are four different acts. Raising a draft is
    // ordinary sales work. **Posting** takes a number nobody can hand back and
    // turns a draft into a document somebody can be sued over. Voiding
    // withdraws one that has already been sent. An organization that gives
    // everybody the first two and nobody the third is expressing something
    // real, and a single "Invoices.Edit" could not.
    PermissionDefinition {
        name: names::SALES,
        display_name: "Sales",
        description: Some("Reach the sales area."),
        parent: Some(names::PAGES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::INVOICES,
        display_name: "Invoices",
        description: Some("View the invoices this workspace has raised."),
        parent: Some(names::SALES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::INVOICES_CREATE,
        display_name: "Create",
        description: Some("Raise a draft invoice."),
        parent: Some(names::INVOICES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::INVOICES_EDIT,
        display_name: "Edit",
        description: Some("Change or delete a draft. A posted invoice cannot be edited."),
        parent: Some(names::INVOICES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::INVOICES_POST,
        display_name: "Post",
        description: Some(
            "Number a draft and issue it. The number cannot be handed back, and the document \
             cannot be edited afterwards.",
        ),
        parent: Some(names::INVOICES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::INVOICES_VOID,
        display_name: "Void",
        description: Some("Withdraw a posted invoice. It keeps its number."),
        parent: Some(names::INVOICES),
        default_for_user: false,
    },
    // -- Master data ------------------------------------------------------
    //
    // Not under Administration, for the reason Files is not: keeping a customer
    // list up to date is ordinary commercial work, and putting it there would
    // mean granting the administration area to everybody in sales. Taxes are
    // the exception within the exception - reading them is ordinary, changing
    // one changes what every future document comes to.
    PermissionDefinition {
        name: names::MASTER,
        display_name: "Master data",
        description: Some("Reach the master data area."),
        parent: Some(names::PAGES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::PARTIES,
        display_name: "Parties",
        description: Some("View the organizations and people this workspace trades with."),
        parent: Some(names::MASTER),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::PARTIES_CREATE,
        display_name: "Create",
        description: Some("Add a party."),
        parent: Some(names::PARTIES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::PARTIES_EDIT,
        display_name: "Edit",
        description: Some("Change a party's details, addresses and contacts."),
        parent: Some(names::PARTIES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::PARTIES_DELETE,
        display_name: "Delete",
        description: Some("Remove a party that no document refers to."),
        parent: Some(names::PARTIES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::TAXES,
        display_name: "Taxes",
        description: Some("View the tax codes, rates and groups this workspace uses."),
        parent: Some(names::MASTER),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::TAXES_EDIT,
        display_name: "Edit",
        // One gate over codes, rates and groups rather than three. They are one
        // act: adding a tax means giving it a rate and putting it in a group,
        // and a grant that allowed two of the three would leave a code nothing
        // can reach.
        description: Some("Change a tax code, its rates, or the groups it belongs to."),
        parent: Some(names::TAXES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::ADMINISTRATION,
        display_name: "Administration",
        description: Some("Reach the administration area."),
        parent: Some(names::PAGES),
        default_for_user: false,
    },
    // -- Users ------------------------------------------------------------
    PermissionDefinition {
        name: names::USERS,
        display_name: "Users",
        description: Some("View the people in this workspace."),
        parent: Some(names::ADMINISTRATION),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::USERS_CREATE,
        display_name: "Create",
        description: Some("Invite new people."),
        parent: Some(names::USERS),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::USERS_EDIT,
        display_name: "Edit",
        description: Some("Change someone's profile, status or roles."),
        parent: Some(names::USERS),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::USERS_DELETE,
        display_name: "Delete",
        description: Some("Remove someone from the workspace."),
        parent: Some(names::USERS),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::USERS_CHANGE_PERMISSIONS,
        display_name: "Change permissions",
        description: Some("Grant or revoke permissions on an individual account."),
        parent: Some(names::USERS),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::USERS_IMPERSONATE,
        display_name: "Impersonate",
        description: Some("Sign in as another user. Every use is audited."),
        parent: Some(names::USERS),
        default_for_user: false,
    },
    // -- Roles ------------------------------------------------------------
    PermissionDefinition {
        name: names::ROLES,
        display_name: "Roles",
        description: Some("View the roles defined by this workspace."),
        parent: Some(names::ADMINISTRATION),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::ROLES_CREATE,
        display_name: "Create",
        description: Some("Define a new role."),
        parent: Some(names::ROLES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::ROLES_EDIT,
        display_name: "Edit",
        description: Some("Rename a role or change who holds it."),
        parent: Some(names::ROLES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::ROLES_DELETE,
        display_name: "Delete",
        description: Some("Remove a role. Static roles cannot be removed."),
        parent: Some(names::ROLES),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::ROLES_CHANGE_PERMISSIONS,
        display_name: "Change permissions",
        description: Some("Change which permissions a role grants."),
        parent: Some(names::ROLES),
        default_for_user: false,
    },
    // -- Everything else --------------------------------------------------
    PermissionDefinition {
        name: names::SETTINGS,
        display_name: "Settings",
        description: Some("Change workspace-wide settings."),
        parent: Some(names::ADMINISTRATION),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::AUDIT_LOGS,
        display_name: "Audit logs",
        description: Some("Read the security and activity trail."),
        parent: Some(names::ADMINISTRATION),
        default_for_user: false,
    },
    // -- Apps -------------------------------------------------------------
    //
    // Seeing the store and changing what the workspace subscribes to are two
    // acts, and the second one is the one with an invoice attached. Somebody
    // should be able to look at what is available and ask for it without being
    // able to sign the organization up for it.
    PermissionDefinition {
        name: names::APPS,
        display_name: "Apps",
        description: Some("See which apps this workspace has, and what else there is."),
        parent: Some(names::ADMINISTRATION),
        default_for_user: false,
    },
    PermissionDefinition {
        name: names::APPS_INSTALL,
        display_name: "Install",
        description: Some(
            "Switch an app on for this workspace, or off again. An app that is off keeps              its data.",
        ),
        parent: Some(names::APPS),
        default_for_user: false,
    },
];

/// Look up a definition by name. `None` means the name is not one this build
/// knows about - typically a grant left behind by an older version.
pub fn definition(name: &str) -> Option<&'static PermissionDefinition> {
    DEFINITIONS.iter().find(|def| def.name == name)
}

pub fn is_defined(name: &str) -> bool {
    definition(name).is_some()
}

/// Direct children of `parent`, or the roots when `parent` is `None`.
pub fn children(parent: Option<&str>) -> impl Iterator<Item = &'static PermissionDefinition> + '_ {
    DEFINITIONS.iter().filter(move |def| def.parent == parent)
}

/// Every ancestor of `name`, outermost first.
///
/// Derived from the dotted name rather than by walking `parent` links, so it
/// works for a name this build has never heard of - which is exactly the case
/// where pruning and revocation need it.
pub fn ancestors(name: &str) -> Vec<&str> {
    name.char_indices()
        .filter(|(_, ch)| *ch == '.')
        .map(|(index, _)| &name[..index])
        .collect()
}

/// Whether `name` sits anywhere beneath `ancestor`.
///
/// Matched on a dot boundary, so `PagesOther` is not a child of `Pages`.
pub fn is_descendant_of(name: &str, ancestor: &str) -> bool {
    // `get` rather than an index: the length test above already makes the
    // position valid, but this crate compiles to wasm, where an out-of-bounds
    // index is not a caught panic but a frozen tab. Asking makes the bound a
    // property of the expression instead of a property of the line above it.
    name.starts_with(ancestor) && name.as_bytes().get(ancestor.len()) == Some(&b'.')
}

/// Whether a string is shaped like a permission name.
///
/// Grants are written by administrators through the role editor, so the value
/// reaching the database is checked rather than assumed.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_definition_has_a_parent_that_exists() {
        for def in DEFINITIONS {
            if let Some(parent) = def.parent {
                assert!(
                    is_defined(parent),
                    "{} declares parent {parent}, which is not defined",
                    def.name
                );
            }
        }
    }

    #[test]
    fn parent_links_agree_with_the_dotted_names() {
        // The two are used interchangeably - `ancestors` reads the string,
        // `children` reads the link - so a disagreement would be a quiet bug.
        for def in DEFINITIONS {
            match def.parent {
                Some(parent) => assert_eq!(
                    ancestors(def.name).last().copied(),
                    Some(parent),
                    "{} has parent {parent} but its name says otherwise",
                    def.name
                ),
                None => assert!(
                    !def.name.contains('.'),
                    "{} has no parent but a dotted name",
                    def.name
                ),
            }
        }
    }

    #[test]
    fn names_are_unique_and_well_formed() {
        let mut seen = BTreeSet::new();
        for def in DEFINITIONS {
            assert!(seen.insert(def.name), "duplicate permission {}", def.name);
            assert!(is_valid_name(def.name), "{} is not a valid name", def.name);
        }
    }

    #[test]
    fn parents_are_declared_before_their_children() {
        // The role editor renders in declaration order and indents by depth, so
        // a child listed before its parent would render under the wrong node.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for def in DEFINITIONS {
            if let Some(parent) = def.parent {
                assert!(
                    seen.contains(parent),
                    "{} appears before its parent {parent}",
                    def.name
                );
            }
            seen.insert(def.name);
        }
    }

    #[test]
    fn ancestors_are_listed_outermost_first() {
        assert_eq!(
            ancestors("Pages.Administration.Users.Create"),
            vec![
                "Pages",
                "Pages.Administration",
                "Pages.Administration.Users"
            ]
        );
        assert!(ancestors("Pages").is_empty());
    }

    #[test]
    fn descendants_are_matched_on_a_dot_boundary() {
        assert!(is_descendant_of("Pages.Administration", "Pages"));
        // The critical case: a name that merely shares a prefix is not a child.
        assert!(!is_descendant_of("PagesOther", "Pages"));
        assert!(!is_descendant_of("Pages", "Pages"));
    }

    #[test]
    fn permission_name_shapes_are_checked() {
        assert!(is_valid_name("Pages.Administration.Users.Create"));
        assert!(is_valid_name("Custom_Feature-2"));
        for bad in [
            "",
            ".Pages",
            "Pages.",
            "Pages..Users",
            "Pages Users",
            "Pages;DROP",
        ] {
            assert!(!is_valid_name(bad), "{bad:?} should be rejected");
        }
        assert!(!is_valid_name(&"a".repeat(129)));
    }

    #[test]
    fn definition_helpers_read_the_tree() {
        let def = definition(names::USERS_CREATE).unwrap();
        assert_eq!(def.leaf(), "Create");
        assert_eq!(def.depth(), 3);

        let admin_children: Vec<&str> = children(Some(names::ADMINISTRATION))
            .map(|def| def.name)
            .collect();
        assert_eq!(
            admin_children,
            vec![
                names::USERS,
                names::ROLES,
                names::SETTINGS,
                names::AUDIT_LOGS,
                names::APPS
            ]
        );

        let roots: Vec<&str> = children(None).map(|def| def.name).collect();
        assert_eq!(roots, vec![names::PAGES]);
    }
}

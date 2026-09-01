//! Which layers a page load actually went through, derived from its stacks.
//!
//! # What this is, and what it is not
//!
//! The spine below is *declared* - it is this workspace's shape, written down
//! once. What is drawn on top of it is *measured*: a layer is lit only when a
//! captured stack put a frame in it, and an edge exists only when two adjacent
//! frames crossed between them. Nothing here infers that a layer "must have"
//! run because the one below it did.
//!
//! That split is the point. A purely measured diagram is honest and
//! unreadable: it changes shape on every request, and a screen that made two
//! calls looks like a broken one. A purely declared diagram is readable and
//! worthless, because it says the same thing whatever the application did.
//! Declaring the frame and measuring the fill gives a picture that is stable
//! enough to learn and still cannot claim something that did not happen.
//!
//! # Where the evidence comes from
//!
//! Two sources, and neither is the database:
//!
//! * **Every recorded event's stack.** [`crate::collect`] walks the stack on
//!   every event, not only sqlx's, so a request that logged and never queried
//!   still draws its path. This is what keeps the diagram from being a picture
//!   of SQL.
//! * **A log line's own `file`.** Free, and present even when
//!   `profiler.backtraces` is off - so a layer can still be lit when no stack
//!   was walked. It gives points, not edges, which is why the diagram degrades
//!   to lit boxes with no arrows rather than to nothing.
//!
//! # The one thing that is not measured
//!
//! Time per layer. sqlx measures its own round trip and nothing measures how
//! long a request spent inside `phonix-services`. Getting that means
//! `#[instrument]` on hundreds of functions, which
//! `docs/adr/0004-development-profiler.md` section 4 rejects. So an edge
//! carries a count, and only the edge into Postgres carries a duration.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::caller::Frame;
use crate::profile::{Kind, Profile, Token, millis};

/// A box in the diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    /// Matches the first path segment of a workspace-relative file, for the
    /// crates. The external systems use a name no crate can collide with.
    pub id: &'static str,
    pub label: &'static str,
    /// What it is, which decides how it is drawn and whether a file list is
    /// meaningful for it.
    pub kind: LayerKind,
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    /// A crate in this workspace. Has files, and can be clicked into.
    Crate,
    /// Something outside the process. Has no files and never will.
    External,
}

/// The declared shape of this workspace, top to bottom.
///
/// Ordering is the request's direction of travel: axum takes the request,
/// Leptos renders or dispatches a server function, an app crate or the service
/// layer answers it, an adapter talks to something outside the process.
///
/// A crate missing from here is not dropped - see [`OTHER`] - because a
/// diagram that silently omits a layer is worse than one that admits it has an
/// "elsewhere" box.
pub const SPINE: &[Layer] = &[
    Layer {
        id: "phonix-server",
        label: "HTTP",
        kind: LayerKind::Crate,
        row: 0,
        column: 0,
    },
    Layer {
        id: "phonix-web",
        label: "Web / server fns",
        kind: LayerKind::Crate,
        row: 1,
        column: 0,
    },
    Layer {
        id: "app-books",
        label: "Books",
        kind: LayerKind::Crate,
        row: 2,
        column: 0,
    },
    Layer {
        id: "phonix-master",
        label: "Master",
        kind: LayerKind::Crate,
        row: 2,
        column: 1,
    },
    Layer {
        id: "phonix-tax",
        label: "Tax",
        kind: LayerKind::Crate,
        row: 2,
        column: 2,
    },
    Layer {
        id: "phonix-services",
        label: "Application",
        kind: LayerKind::Crate,
        row: 3,
        column: 0,
    },
    Layer {
        id: "phonix-db",
        label: "Data access",
        kind: LayerKind::Crate,
        row: 4,
        column: 0,
    },
    Layer {
        id: "phonix-cache",
        label: "Cache",
        kind: LayerKind::Crate,
        row: 4,
        column: 1,
    },
    Layer {
        id: "phonix-storage",
        label: "Storage",
        kind: LayerKind::Crate,
        row: 4,
        column: 2,
    },
    Layer {
        id: "phonix-messaging",
        label: "Messaging",
        kind: LayerKind::Crate,
        row: 4,
        column: 3,
    },
    Layer {
        id: "postgres",
        label: "Postgres",
        kind: LayerKind::External,
        row: 5,
        column: 0,
    },
    Layer {
        id: "redis",
        label: "Redis",
        kind: LayerKind::External,
        row: 5,
        column: 1,
    },
    Layer {
        id: "object-store",
        label: "Object store",
        kind: LayerKind::External,
        row: 5,
        column: 2,
    },
    Layer {
        id: "rabbitmq",
        label: "RabbitMQ",
        kind: LayerKind::External,
        row: 5,
        column: 3,
    },
];

/// Where a workspace crate that is not on the spine is filed.
///
/// `phonix-core`, `phonix-config` and `phonix-telemetry` are real frames on
/// real stacks and they belong to no band. Putting them in a named box is
/// honest; dropping them would quietly break the chain and draw an edge
/// between two layers that do not call each other directly.
pub const OTHER: Layer = Layer {
    id: "other",
    label: "Shared",
    kind: LayerKind::Crate,
    row: 3,
    column: 1,
};

/// The only external system whose operations are reported to us.
///
/// sqlx logs every statement, which is why Postgres can be lit from evidence.
/// No other adapter announces its round trips, so Redis, the object store and
/// RabbitMQ stay unlit however much traffic they carry - and the report says
/// so rather than leaving a developer to wonder.
const DATABASE: &str = "postgres";

/// A layer as this page load found it.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub kind: LayerKind,
    pub row: usize,
    pub column: usize,
    /// Whether anything put this layer on a stack. Everything else is drawn
    /// greyed.
    pub observed: bool,
    /// Frames landing in this layer, across every event in scope.
    pub hits: usize,
    /// The files behind those frames, busiest first. Empty for an external.
    pub files: Vec<FileHit>,
}

/// One file of one layer, and the lines within it that were on a stack.
#[derive(Debug, Clone, Serialize)]
pub struct FileHit {
    /// Workspace-relative, so it is a path in the repository rather than a
    /// path on the machine that built it.
    pub file: String,
    pub hits: usize,
    pub functions: Vec<FunctionHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionHit {
    pub function: String,
    pub line: u32,
    pub hits: usize,
}

/// One layer calling into the next.
#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// How many stacks crossed here.
    pub hits: usize,
    /// Time spent, for the one edge where time is known: into Postgres.
    ///
    /// `None` everywhere else, and drawn as nothing rather than as zero. See
    /// the module note on why there is no per-layer timing.
    #[serde(serialize_with = "millis_opt")]
    pub elapsed: Option<Duration>,
}

/// The graph for one scope - a whole page load, or one request within it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Flow {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Statements seen in this scope, which is what lights Postgres.
    pub statements: usize,
    /// True when nothing carried a stack, so the diagram has boxes and no
    /// arrows. Almost always means `profiler.backtraces` is off.
    pub without_stacks: bool,
}

/// One request in the page load, with its own graph.
#[derive(Debug, Clone, Serialize)]
pub struct Phase {
    pub token: Token,
    pub kind: Kind,
    pub method: String,
    pub route: String,
    pub status: u16,
    pub flow: Flow,
}

/// A page load: the whole thing, and each request in it.
#[derive(Debug, Clone, Serialize)]
pub struct PageFlow {
    pub overall: Flow,
    pub phases: Vec<Phase>,
}

impl PageFlow {
    /// Build the graph for a page load, oldest request first.
    pub fn of(profiles: &[Arc<Profile>]) -> Self {
        Self {
            overall: Flow::of(profiles),
            phases: profiles
                .iter()
                .map(|profile| Phase {
                    token: profile.token,
                    kind: profile.kind,
                    method: profile.method.clone(),
                    route: profile.route_or_path().to_owned(),
                    status: profile.status,
                    flow: Flow::of(std::slice::from_ref(profile)),
                })
                .collect(),
        }
    }
}

impl Flow {
    /// Fill the declared spine from what these profiles recorded.
    pub fn of(profiles: &[Arc<Profile>]) -> Self {
        let mut builder = Builder::default();

        for profile in profiles {
            for query in &profile.queries {
                let frames = query.caller.resolve();

                builder.walk(&frames);
                // The one edge with a duration on it: whatever ran the
                // statement, into the database.
                builder.terminate(&frames, DATABASE, query.elapsed);
                builder.statements += 1;
            }

            for log in &profile.logs {
                let frames = log.caller.resolve();

                builder.walk(&frames);

                // Even with no stack the line still names its own file, so the
                // layer is lit and only the arrows are missing.
                if frames.is_empty()
                    && let Some(source) = &log.source
                {
                    builder.note(source, log.message.as_str(), log.line.unwrap_or(0));
                }
            }
        }

        builder.finish()
    }

    /// The node for a layer id, if the spine has one.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// Which layer a workspace-relative path belongs to.
///
/// `phonix-db/src/tenancy/catalog.rs` is `phonix-db`. A path from a crate that
/// is not on the spine lands in [`OTHER`] rather than being dropped.
pub fn layer_of(file: &str) -> Layer {
    let crate_name = file.split('/').next().unwrap_or_default();

    SPINE
        .iter()
        .find(|layer| layer.kind == LayerKind::Crate && layer.id == crate_name)
        .copied()
        .unwrap_or(OTHER)
}

/// Accumulates hits and crossings before they are turned into a [`Flow`].
#[derive(Debug, Default)]
struct Builder {
    /// `(layer id, file) -> (hits, functions)`, kept flat because the spine is
    /// a dozen entries and a map would cost more than it saves.
    files: Vec<(&'static str, String, usize, Vec<FunctionHit>)>,
    hits: Vec<(&'static str, usize)>,
    edges: Vec<Edge>,
    statements: usize,
    stacks: usize,
}

impl Builder {
    /// Attribute a stack: every frame lights its layer, every crossing between
    /// two different layers is an edge.
    fn walk(&mut self, frames: &[Frame]) {
        if frames.is_empty() {
            return;
        }

        self.stacks += 1;

        for frame in frames {
            self.hit(&frame.file, &frame.function, frame.line);
        }

        // `resolve` returns innermost first, so walking backwards is walking
        // in the direction the calls actually went.
        let mut previous: Option<&'static str> = None;

        for frame in frames.iter().rev() {
            let layer = layer_of(&frame.file).id;

            match previous {
                // Consecutive frames in one layer are one layer, not a
                // self-edge. A service calling six of its own functions is not
                // six crossings.
                Some(from) if from == layer => {}
                Some(from) => self.cross(from, layer, None),
                None => {}
            }

            previous = Some(layer);
        }
    }

    /// The edge from wherever a stack ended into an external system.
    fn terminate(&mut self, frames: &[Frame], external: &'static str, elapsed: Option<Duration>) {
        // The innermost workspace frame is what called out of the process. With
        // no frames at all the statement still happened, so the external is lit
        // from a source that is not a stack.
        let from = frames
            .first()
            .map(|frame| layer_of(&frame.file).id)
            .unwrap_or(OTHER.id);

        self.bump(external);
        self.cross(from, external, elapsed);
    }

    /// A log line with no stack: the layer is known, the path to it is not.
    fn note(&mut self, file: &str, function: &str, line: u32) {
        self.hit(file, function, line);
    }

    fn hit(&mut self, file: &str, function: &str, line: u32) {
        let layer = layer_of(file).id;

        self.bump(layer);

        let entry = match self
            .files
            .iter_mut()
            .find(|(id, seen, _, _)| *id == layer && seen == file)
        {
            Some(entry) => entry,
            None => {
                self.files.push((layer, file.to_owned(), 0, Vec::new()));

                self.files.last_mut().expect("just pushed")
            }
        };

        entry.2 += 1;

        match entry
            .3
            .iter_mut()
            .find(|hit| hit.function == function && hit.line == line)
        {
            Some(hit) => hit.hits += 1,
            None => entry.3.push(FunctionHit {
                function: function.to_owned(),
                line,
                hits: 1,
            }),
        }
    }

    fn bump(&mut self, layer: &'static str) {
        match self.hits.iter_mut().find(|(id, _)| *id == layer) {
            Some((_, count)) => *count += 1,
            None => self.hits.push((layer, 1)),
        }
    }

    fn cross(&mut self, from: &'static str, to: &'static str, elapsed: Option<Duration>) {
        let found = self
            .edges
            .iter_mut()
            .find(|edge| edge.from == from && edge.to == to);

        match found {
            Some(edge) => {
                edge.hits += 1;

                if let Some(taken) = elapsed {
                    *edge.elapsed.get_or_insert(Duration::ZERO) += taken;
                }
            }
            None => self.edges.push(Edge {
                from: from.to_owned(),
                to: to.to_owned(),
                hits: 1,
                elapsed,
            }),
        }
    }

    fn finish(mut self) -> Flow {
        let mut nodes = Vec::with_capacity(SPINE.len());

        for layer in SPINE.iter().chain(std::iter::once(&OTHER)) {
            let hits = self
                .hits
                .iter()
                .find(|(id, _)| *id == layer.id)
                .map(|(_, count)| *count)
                .unwrap_or(0);

            let mut files: Vec<FileHit> = self
                .files
                .iter()
                .filter(|(id, ..)| *id == layer.id)
                .map(|(_, file, hits, functions)| FileHit {
                    file: file.clone(),
                    hits: *hits,
                    functions: {
                        let mut functions = functions.clone();
                        functions.sort_by(|a, b| b.hits.cmp(&a.hits).then(a.line.cmp(&b.line)));
                        functions
                    },
                })
                .collect();

            files.sort_by(|a, b| b.hits.cmp(&a.hits).then(a.file.cmp(&b.file)));

            // `Shared` earns its box only when something landed in it. Drawn
            // always, it would be a permanent empty square in the middle of
            // the diagram.
            if layer.id == OTHER.id && hits == 0 {
                continue;
            }

            nodes.push(Node {
                id: layer.id.to_owned(),
                label: layer.label.to_owned(),
                kind: layer.kind,
                row: layer.row,
                column: layer.column,
                observed: hits > 0,
                hits,
                files,
            });
        }

        self.edges.sort_by_key(|edge| std::cmp::Reverse(edge.hits));

        Flow {
            nodes,
            edges: std::mem::take(&mut self.edges),
            statements: self.statements,
            without_stacks: self.stacks == 0,
        }
    }
}

/// Milliseconds, or nothing. Matches how the rest of the report serialises a
/// duration - see [`crate::profile::millis`].
fn millis_opt<S: serde::Serializer>(
    value: &Option<Duration>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(duration) => millis(duration, serializer),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(file: &str, function: &str, line: u32) -> Frame {
        Frame {
            function: function.to_owned(),
            file: file.to_owned(),
            line,
        }
    }

    /// Innermost first, the way `Caller::resolve` hands them over.
    fn stack() -> Vec<Frame> {
        vec![
            frame("phonix-db/src/tenancy/catalog.rs", "find_by_slug", 156),
            frame("phonix-services/src/tenancy.rs", "resolve", 41),
            frame("phonix-server/src/middleware.rs", "resolve_tenant", 31),
        ]
    }

    #[test]
    fn a_file_is_filed_under_its_crate() {
        assert_eq!(layer_of("phonix-db/src/tenancy/catalog.rs").id, "phonix-db");
        assert_eq!(layer_of("phonix-web/src/app.rs").id, "phonix-web");
    }

    /// A crate that is not on the spine must still land somewhere. Dropping it
    /// would join two layers that never call each other directly.
    #[test]
    fn a_crate_off_the_spine_lands_in_shared() {
        assert_eq!(layer_of("phonix-core/src/error.rs").id, "other");
        assert_eq!(layer_of("phonix-telemetry/src/lib.rs").id, "other");
    }

    #[test]
    fn a_stack_lights_every_layer_it_passed_through() {
        let mut builder = Builder::default();
        builder.walk(&stack());
        let flow = builder.finish();

        for id in ["phonix-server", "phonix-services", "phonix-db"] {
            assert!(
                flow.node(id).expect("on the spine").observed,
                "{id} was on the stack"
            );
        }

        assert!(
            !flow.node("phonix-web").expect("on the spine").observed,
            "nothing put phonix-web on this stack, so it must stay grey"
        );
    }

    /// The direction has to be the direction the calls went, or the diagram is
    /// drawn upside down and reads as the database calling the handler.
    #[test]
    fn edges_run_from_caller_to_callee() {
        let mut builder = Builder::default();
        builder.walk(&stack());
        let flow = builder.finish();

        let pairs: Vec<(String, String)> = flow
            .edges
            .iter()
            .map(|edge| (edge.from.clone(), edge.to.clone()))
            .collect();

        assert!(pairs.contains(&("phonix-server".to_owned(), "phonix-services".to_owned())));
        assert!(pairs.contains(&("phonix-services".to_owned(), "phonix-db".to_owned())));
        assert!(
            !pairs.contains(&("phonix-db".to_owned(), "phonix-services".to_owned())),
            "an edge pointing back up means the stack was read the wrong way"
        );
    }

    /// A service calling six of its own functions is one layer, not six
    /// crossings, or the busiest edge in every diagram is a self-edge.
    #[test]
    fn consecutive_frames_in_one_layer_are_not_an_edge() {
        let frames = vec![
            frame("phonix-services/src/a.rs", "one", 1),
            frame("phonix-services/src/b.rs", "two", 2),
            frame("phonix-services/src/c.rs", "three", 3),
        ];

        let mut builder = Builder::default();
        builder.walk(&frames);
        let flow = builder.finish();

        assert!(
            flow.edges.is_empty(),
            "a stack inside one layer crosses nothing"
        );
        assert_eq!(flow.node("phonix-services").expect("on the spine").hits, 3);
    }

    /// The whole reason the capture point moved off sqlx: a request that only
    /// logged still has to draw its path.
    #[test]
    fn a_stack_with_no_statement_still_draws_a_path() {
        let mut builder = Builder::default();
        builder.walk(&[
            frame("phonix-cache/src/lib.rs", "get", 12),
            frame("phonix-services/src/billing.rs", "quote", 88),
        ]);
        let flow = builder.finish();

        assert_eq!(flow.statements, 0, "nothing queried");
        assert!(flow.node("phonix-cache").expect("on the spine").observed);
        assert_eq!(
            flow.edges.len(),
            1,
            "services -> cache is a real edge with no SQL anywhere near it"
        );
    }

    /// Only the database edge carries time, because it is the only one that is
    /// measured. See the module note.
    #[test]
    fn only_the_database_edge_carries_a_duration() {
        let frames = stack();
        let mut builder = Builder::default();

        builder.walk(&frames);
        builder.terminate(&frames, DATABASE, Some(Duration::from_millis(9)));

        let flow = builder.finish();
        let into_db = flow
            .edges
            .iter()
            .find(|edge| edge.to == DATABASE)
            .expect("the statement produced an edge into postgres");

        assert_eq!(into_db.from, "phonix-db");
        assert_eq!(into_db.elapsed, Some(Duration::from_millis(9)));

        for edge in flow.edges.iter().filter(|edge| edge.to != DATABASE) {
            assert!(
                edge.elapsed.is_none(),
                "{} -> {} has no measured time and must not claim any",
                edge.from,
                edge.to
            );
        }
    }

    /// The other three externals have no adapter that reports round trips, so
    /// they must stay grey rather than being lit by association with the crate
    /// that talks to them.
    #[test]
    fn an_external_nobody_measured_stays_grey() {
        let mut builder = Builder::default();
        builder.walk(&[
            frame("phonix-cache/src/lib.rs", "get", 12),
            frame("phonix-services/src/billing.rs", "quote", 88),
        ]);
        let flow = builder.finish();

        for id in ["redis", "object-store", "rabbitmq"] {
            assert!(
                !flow.node(id).expect("on the spine").observed,
                "{id} is not measured and must not be lit by its adapter running"
            );
        }
    }

    #[test]
    fn files_carry_the_functions_and_lines_behind_them() {
        let mut builder = Builder::default();
        builder.walk(&stack());
        builder.walk(&stack());

        let flow = builder.finish();
        let db = flow.node("phonix-db").expect("on the spine");
        let file = db.files.first().expect("one file was on the stack");

        assert_eq!(file.file, "phonix-db/src/tenancy/catalog.rs");
        assert_eq!(file.hits, 2);
        assert_eq!(file.functions.len(), 1);
        assert_eq!(file.functions[0].line, 156);
        assert_eq!(file.functions[0].hits, 2);
    }

    /// With `backtraces` off there are no stacks at all. The diagram has to say
    /// so, because boxes with no arrows otherwise read as "nothing called
    /// anything".
    #[test]
    fn a_flow_with_no_stacks_says_so() {
        assert!(Builder::default().finish().without_stacks);
    }
}

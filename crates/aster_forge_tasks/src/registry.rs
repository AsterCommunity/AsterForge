//! Product-facing task registry support traits.

/// Minimal read-only view over a persisted task row.
///
/// Product crates implement this for their database model so Forge can decode typed payloads and
/// results without depending on a concrete ORM entity or schema extension columns.
pub trait TaskRecord<Kind> {
    /// Stable database identifier used in diagnostics.
    fn id(&self) -> i64;

    /// Product-owned task kind enum.
    fn kind(&self) -> Kind;

    /// Stored JSON payload.
    fn payload_json(&self) -> &str;

    /// Stored JSON result, if the task has completed with one.
    fn result_json(&self) -> Option<&str>;
}

/// Generates a product-local static task registry.
///
/// The macro intentionally requires an explicit `kind => static_adapter` mapping. That keeps task
/// registration auditable in product crates while removing the repetitive match forwarding that
/// otherwise drifts between `spec_for_kind`, `task_lane`, and `task_lane_kinds`.
#[macro_export]
macro_rules! task_registry {
    (
        $(#[$meta:meta])*
        $vis:vis mod $module:ident {
            state: $state:ty;
            task: $task:ty;
            config: $config:ty;
            context: $context:ty;
            error: $error:ty;
            kind: $kind:ty;
            lane: $lane:ty;
            payload: $payload:ty;
            result: $result:ty;
            specs {
                $(
                    $adapter:ident: $spec:ty => $kind_value:path
                ),+ $(,)?
            }
            lanes {
                $(
                    $lane_value:path => [$($lane_kind:path),* $(,)?]
                ),+ $(,)?
            }
        }
    ) => {
        $(#[$meta])*
        $vis mod $module {
            $(
                static $adapter: $crate::TaskSpecAdapter<$spec> =
                    $crate::TaskSpecAdapter::new();
            )+

            /// Returns the registered task spec for a product task kind.
            pub fn spec_for_kind(
                kind: $kind,
            ) -> &'static dyn $crate::ErasedBackgroundTaskSpec<
                $state,
                $task,
                $config,
                $context,
                $kind,
                $lane,
                $payload,
                $result,
                $error,
            > {
                match kind {
                    $(
                        $kind_value => &$adapter,
                    )+
                }
            }

            /// Returns the lane for a product task kind.
            pub fn task_lane(kind: $kind) -> $lane {
                spec_for_kind(kind).lane()
            }

            /// Returns all task kinds configured for a lane.
            pub fn task_lane_kinds(lane: $lane) -> &'static [$kind] {
                match lane {
                    $(
                        $lane_value => &[$($lane_kind),*],
                    )+
                }
            }

            #[cfg(test)]
            mod registry_tests {
                use super::*;

                #[test]
                fn registered_task_lanes_are_bidirectionally_consistent() {
                    $(
                        let lane = task_lane($kind_value);
                        assert!(
                            task_lane_kinds(lane).contains(&$kind_value),
                            "lane {lane:?} does not list task kind {:?}",
                            $kind_value
                        );
                    )+
                }
            }
        }
    };
}

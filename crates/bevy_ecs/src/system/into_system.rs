use crate::{Commands, Resources, System, SystemId, SystemParam, ThreadLocalExecution};
use bevy_hecs::{ArchetypeComponent, QueryAccess, TypeAccess, World};
use parking_lot::Mutex;
use std::{any::TypeId, borrow::Cow, sync::Arc};

pub struct SystemState {
    pub(crate) id: SystemId,
    pub(crate) name: Cow<'static, str>,
    pub(crate) is_initialized: bool,
    pub(crate) archetype_component_access: TypeAccess<ArchetypeComponent>,
    pub(crate) resource_access: TypeAccess<TypeId>,
    pub(crate) query_archetype_component_accesses: Vec<TypeAccess<ArchetypeComponent>>,
    pub(crate) query_accesses: Vec<Vec<QueryAccess>>,
    pub(crate) query_type_names: Vec<&'static str>,
    pub(crate) commands: Commands,
    pub(crate) arc_commands: Option<Arc<Mutex<Commands>>>,
    pub(crate) current_query_index: usize,
}

impl SystemState {
    pub fn reset_indices(&mut self) {
        self.current_query_index = 0;
    }

    pub fn update(&mut self, world: &World) {
        self.archetype_component_access.clear();
        let mut conflict_index = None;
        let mut conflict_name = None;
        for (i, (query_accesses, component_access)) in self
            .query_accesses
            .iter()
            .zip(self.query_archetype_component_accesses.iter_mut())
            .enumerate()
        {
            component_access.clear();
            for query_access in query_accesses.iter() {
                query_access.get_world_archetype_access(world, Some(component_access));
            }
            if !component_access.is_compatible(&self.archetype_component_access) {
                conflict_index = Some(i);
                conflict_name = component_access
                    .get_conflict(&self.archetype_component_access)
                    .and_then(|archetype_component| {
                        query_accesses
                            .iter()
                            .filter_map(|query_access| {
                                query_access.get_type_name(archetype_component.component)
                            })
                            .next()
                    });
                break;
            }
            self.archetype_component_access.union(component_access);
        }
        if let Some(conflict_index) = conflict_index {
            let mut conflicts_with_index = None;
            for prior_index in 0..conflict_index {
                if !self.query_archetype_component_accesses[conflict_index]
                    .is_compatible(&self.query_archetype_component_accesses[prior_index])
                {
                    conflicts_with_index = Some(prior_index);
                }
            }
            panic!("System {} has conflicting queries. {} conflicts with the component access [{}] in this prior query: {}",
                self.name,
                self.query_type_names[conflict_index],
                conflict_name.unwrap_or("Unknown"),
                conflicts_with_index.map(|index| self.query_type_names[index]).unwrap_or("Unknown"));
        }
    }
}

pub struct FuncSystem<F, Init, ThreadLocalFunc>
where
    F: FnMut(&mut SystemState, &World, &Resources) + Send + Sync + 'static,
    Init: FnMut(&mut SystemState, &World, &mut Resources) + Send + Sync + 'static,
    ThreadLocalFunc: FnMut(&mut SystemState, &mut World, &mut Resources) + Send + Sync + 'static,
{
    func: F,
    thread_local_func: ThreadLocalFunc,
    init_func: Init,
    state: SystemState,
}

impl<F, Init, ThreadLocalFunc> System for FuncSystem<F, Init, ThreadLocalFunc>
where
    F: FnMut(&mut SystemState, &World, &Resources) + Send + Sync + 'static,
    Init: FnMut(&mut SystemState, &World, &mut Resources) + Send + Sync + 'static,
    ThreadLocalFunc: FnMut(&mut SystemState, &mut World, &mut Resources) + Send + Sync + 'static,
{
    fn name(&self) -> std::borrow::Cow<'static, str> {
        self.state.name.clone()
    }

    fn id(&self) -> SystemId {
        self.state.id
    }

    fn update(&mut self, world: &World) {
        self.state.update(world);
    }

    fn archetype_component_access(&self) -> &TypeAccess<ArchetypeComponent> {
        &self.state.archetype_component_access
    }

    fn resource_access(&self) -> &TypeAccess<std::any::TypeId> {
        &self.state.resource_access
    }

    fn thread_local_execution(&self) -> ThreadLocalExecution {
        ThreadLocalExecution::NextFlush
    }

    fn run(&mut self, world: &World, resources: &Resources) {
        (self.func)(&mut self.state, world, resources)
    }

    fn run_thread_local(&mut self, world: &mut World, resources: &mut Resources) {
        (self.thread_local_func)(&mut self.state, world, resources)
    }

    fn initialize(&mut self, world: &mut World, resources: &mut Resources) {
        (self.init_func)(&mut self.state, world, resources);
        self.state.is_initialized = true;
    }

    fn is_initialized(&self) -> bool {
        self.state.is_initialized
    }
}

pub trait IntoSystem<Params> {
    fn system(self) -> Box<dyn System>;
}

macro_rules! impl_into_system {
    ($($param: ident),*) => {
        impl<Func, $($param: SystemParam),*> IntoSystem<($($param,)*)> for Func
        where Func: FnMut($($param),*) + Send + Sync + 'static,
        {
            #[allow(unused_variables)]
            #[allow(unused_unsafe)]
            #[allow(non_snake_case)]
            fn system(mut self) -> Box<dyn System> {
                Box::new(FuncSystem {
                    state: SystemState {
                        name: std::any::type_name::<Self>().into(),
                        archetype_component_access: TypeAccess::default(),
                        resource_access: TypeAccess::default(),
                        is_initialized: false,
                        id: SystemId::new(),
                        commands: Commands::default(),
                        arc_commands: Default::default(),
                        query_archetype_component_accesses: Vec::new(),
                        query_accesses: Vec::new(),
                        query_type_names: Vec::new(),
                        current_query_index: 0,
                    },
                    func: move |state, world, resources| {
                        state.reset_indices();
                        unsafe {
                            if let Some(($($param,)*)) = <($($param,)*)>::get_param(state, world, resources) {
                                self($($param),*);
                            }
                        }
                    },
                    thread_local_func: |state, world, resources| {
                        state.commands.apply(world, resources);
                        if let Some(ref commands) = state.arc_commands {
                            let mut commands = commands.lock();
                            commands.apply(world, resources);
                        }
                    },
                    init_func: |state, world, resources| {
                        $($param::init(state, world, resources);)*
                    },
                })
            }
        }

    };
}

impl_into_system!();
impl_into_system!(A);
impl_into_system!(A, B);
impl_into_system!(A, B, C);
impl_into_system!(A, B, C, D);
impl_into_system!(A, B, C, D, E);
impl_into_system!(A, B, C, D, E, F);
impl_into_system!(A, B, C, D, E, F, G);
impl_into_system!(A, B, C, D, E, F, G, H);
impl_into_system!(A, B, C, D, E, F, G, H, I);
impl_into_system!(A, B, C, D, E, F, G, H, I, J);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_into_system!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

#[cfg(test)]
mod tests {
    use super::IntoSystem;
    use crate::{
        resource::{ResMut, Resources},
        schedule::Schedule,
        ChangedRes, Query, QuerySet, System,
    };
    use bevy_hecs::{Entity, Or, With, World};

    #[derive(Debug, Eq, PartialEq)]
    struct A;
    struct B;
    struct C;
    struct D;

    #[test]
    fn query_system_gets() {
        fn query_system(
            mut ran: ResMut<bool>,
            entity_query: Query<Entity, With<A>>,
            b_query: Query<&B>,
            a_c_query: Query<(&A, &C)>,
            d_query: Query<&D>,
        ) {
            let entities = entity_query.iter().collect::<Vec<Entity>>();
            assert!(
                b_query.get_component::<B>(entities[0]).is_err(),
                "entity 0 should not have B"
            );
            assert!(
                b_query.get_component::<B>(entities[1]).is_ok(),
                "entity 1 should have B"
            );
            assert!(
                b_query.get_component::<A>(entities[1]).is_err(),
                "entity 1 should have A, but b_query shouldn't have access to it"
            );
            assert!(
                b_query.get_component::<D>(entities[3]).is_err(),
                "entity 3 should have D, but it shouldn't be accessible from b_query"
            );
            assert!(
                b_query.get_component::<C>(entities[2]).is_err(),
                "entity 2 has C, but it shouldn't be accessible from b_query"
            );
            assert!(
                a_c_query.get_component::<C>(entities[2]).is_ok(),
                "entity 2 has C, and it should be accessible from a_c_query"
            );
            assert!(
                a_c_query.get_component::<D>(entities[3]).is_err(),
                "entity 3 should have D, but it shouldn't be accessible from b_query"
            );
            assert!(
                d_query.get_component::<D>(entities[3]).is_ok(),
                "entity 3 should have D"
            );

            *ran = true;
        }

        let mut world = World::default();
        let mut resources = Resources::default();
        resources.insert(false);
        world.spawn((A,));
        world.spawn((A, B));
        world.spawn((A, C));
        world.spawn((A, D));

        run_system(&mut world, &mut resources, query_system.system());

        assert!(*resources.get::<bool>().unwrap(), "system ran");
    }

    #[test]
    fn or_query_set_system() {
        // Regression test for issue #762
        use crate::{Added, Changed, Mutated, Or};
        let query_system = move |mut ran: ResMut<bool>,
                                 set: QuerySet<(
            Query<(), Or<(Changed<A>, Changed<B>)>>,
            Query<(), Or<(Added<A>, Added<B>)>>,
            Query<(), Or<(Mutated<A>, Mutated<B>)>>,
        )>| {
            let changed = set.q0().iter().count();
            let added = set.q1().iter().count();
            let mutated = set.q2().iter().count();

            assert_eq!(changed, 1);
            assert_eq!(added, 1);
            assert_eq!(mutated, 0);

            *ran = true;
        };

        let mut world = World::default();
        let mut resources = Resources::default();
        resources.insert(false);
        world.spawn((A, B));

        run_system(&mut world, &mut resources, query_system.system());

        assert!(*resources.get::<bool>().unwrap(), "system ran");
    }

    #[test]
    fn changed_resource_system() {
        fn incr_e_on_flip(_run_on_flip: ChangedRes<bool>, mut query: Query<&mut i32>) {
            for mut i in query.iter_mut() {
                *i += 1;
            }
        }

        let mut world = World::default();
        let mut resources = Resources::default();
        resources.insert(false);
        let ent = world.spawn((0,));

        let mut schedule = Schedule::default();
        schedule.add_stage("update");
        schedule.add_system_to_stage("update", incr_e_on_flip.system());
        schedule.initialize(&mut world, &mut resources);

        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 1);

        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 1);

        *resources.get_mut::<bool>().unwrap() = true;
        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 2);
    }

    #[test]
    fn changed_resource_or_system() {
        fn incr_e_on_flip(
            _or: Or<(Option<ChangedRes<bool>>, Option<ChangedRes<i32>>)>,
            mut query: Query<&mut i32>,
        ) {
            for mut i in query.iter_mut() {
                *i += 1;
            }
        }

        let mut world = World::default();
        let mut resources = Resources::default();
        resources.insert(false);
        resources.insert::<i32>(10);
        let ent = world.spawn((0,));

        let mut schedule = Schedule::default();
        schedule.add_stage("update");
        schedule.add_system_to_stage("update", incr_e_on_flip.system());
        schedule.initialize(&mut world, &mut resources);

        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 1);

        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 1);

        *resources.get_mut::<bool>().unwrap() = true;
        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 2);

        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 2);

        *resources.get_mut::<i32>().unwrap() = 20;
        schedule.run(&mut world, &mut resources);
        assert_eq!(*(world.get::<i32>(ent).unwrap()), 3);
    }

    #[test]
    #[should_panic]
    fn conflicting_query_mut_system() {
        fn sys(_q1: Query<&mut A>, _q2: Query<&mut A>) {}

        let mut world = World::default();
        let mut resources = Resources::default();
        world.spawn((A,));

        run_system(&mut world, &mut resources, sys.system());
    }

    #[test]
    #[should_panic]
    fn conflicting_query_immut_system() {
        fn sys(_q1: Query<&A>, _q2: Query<&mut A>) {}

        let mut world = World::default();
        let mut resources = Resources::default();
        world.spawn((A,));

        run_system(&mut world, &mut resources, sys.system());
    }

    #[test]
    fn query_set_system() {
        fn sys(_set: QuerySet<(Query<&mut A>, Query<&B>)>) {}

        let mut world = World::default();
        let mut resources = Resources::default();
        world.spawn((A,));

        run_system(&mut world, &mut resources, sys.system());
    }

    #[test]
    #[should_panic]
    fn conflicting_query_with_query_set_system() {
        fn sys(_query: Query<&mut A>, _set: QuerySet<(Query<&mut A>, Query<&B>)>) {}

        let mut world = World::default();
        let mut resources = Resources::default();
        world.spawn((A,));

        run_system(&mut world, &mut resources, sys.system());
    }

    #[test]
    #[should_panic]
    fn conflicting_query_sets_system() {
        fn sys(_set_1: QuerySet<(Query<&mut A>,)>, _set_2: QuerySet<(Query<&mut A>, Query<&B>)>) {}

        let mut world = World::default();
        let mut resources = Resources::default();
        world.spawn((A,));
        run_system(&mut world, &mut resources, sys.system());
    }

    fn run_system(world: &mut World, resources: &mut Resources, system: Box<dyn System>) {
        let mut schedule = Schedule::default();
        schedule.add_stage("update");
        schedule.add_system_to_stage("update", system);

        schedule.initialize(world, resources);
        schedule.run(world, resources);
    }
}

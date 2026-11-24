use std::sync::Arc;

use forall_extension_api::ExtensionEventSink;
use forall_protocol::protocol::Event;
use forall_protocol::protocol::EventMsg;
use forall_protocol::protocol::ThreadGoal;
use forall_protocol::protocol::ThreadGoalUpdatedEvent;

#[derive(Clone)]
pub(crate) struct GoalEventEmitter {
    sink: Arc<dyn ExtensionEventSink>,
}

impl GoalEventEmitter {
    pub(crate) fn new(sink: Arc<dyn ExtensionEventSink>) -> Self {
        Self { sink }
    }

    pub(crate) fn thread_goal_updated(
        &self,
        event_id: impl Into<String>,
        turn_id: Option<String>,
        goal: ThreadGoal,
    ) {
        self.sink.emit(Event {
            id: event_id.into(),
            msg: EventMsg::ThreadGoalUpdated(ThreadGoalUpdatedEvent {
                thread_id: goal.thread_id,
                turn_id,
                goal,
            }),
        });
    }
}

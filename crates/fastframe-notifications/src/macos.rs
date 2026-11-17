//! macOS adapter. The host owns its signed bundle and pumps the main run loop.
//! This module owns the process-wide notification delegate/category registry.
//! It never changes the bundle identity and never requests permission on send.
//!
//! No AppKit application or window is required; signed headless hosts must pump
//! the main loop, for example with mac_usernotifications::block_on_main.
//! Buttonless notifications have no category: their dismissal never requests a
//! host relaunch. Named action categories retain native CustomDismissAction.
//! Buttonless dismissal requires a previously observed delivery, then two absent
//! batches. A notification dismissed before its first observation cannot be
//! distinguished from OS-deferred delivery, so no dismissal is inferred for it.
//! Limits: 256 observations process-wide, 128 action configurations (active ones
//! pinned). Older released configurations may lose their buttons after eviction.
//! macOS notification history is preserved on drop; explicit close/withdraw removes it.
// SAFETY: This platform boundary implements Apple's delegate ABI and reads
// callback pointers valid for the callback lifetime. Objective-C objects use
// Retained ownership; shared callback state is protected by Mutex.
#![allow(
    unsafe_code,
    reason = "Objective-C delegate ABI and callback pointers are confined to this native adapter"
)]

use crate::{App, Error, Event, Notification, Permission, Sound, worker};
use block2::RcBlock;
use objc2::{
    AnyThread, define_class,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSSet, NSString, NSUUID,
};
use objc2_user_notifications::*;
use std::{
    collections::{HashMap, HashSet},
    ptr::NonNull,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

pub(crate) struct Native;
const GENERATION: &str = "fastframe.notification.generation";
const ACTION_PREFIX: &str = "fastframe.action.";
const MAX_PENDING: usize = 256;
const MAX_CATEGORIES: usize = 128;
const POLL_INTERVAL: Duration = Duration::from_millis(500);
type PollSnapshot = Vec<(String, String, u64)>;
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

struct Pending {
    generation: String,
    sender: Option<oneshot::Sender<Event>>,
    actions: Vec<String>,
    category: String,
    revision: u64,
    ready: bool,
    observed: bool,
    missing: u8,
}
struct Category {
    id: String,
    actions: Vec<(String, String)>,
}
#[derive(Default)]
struct Registry {
    pending: HashMap<String, Pending>,
    // LRU cache retains buttons for recently released notifications, without
    // accumulating every action label ever used. Active categories are pinned.
    categories: Vec<Category>,
    poll_in_flight: bool,
    next_poll: Option<Instant>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
fn named_actions(notification: &Notification) -> Vec<(String, String)> {
    notification
        .actions
        .iter()
        .filter(|(id, _)| id != "default")
        .cloned()
        .collect()
}
impl Registry {
    fn begin_poll(&mut self, now: Instant) -> Option<PollSnapshot> {
        if self.poll_in_flight || self.next_poll.is_some_and(|next| now < next) {
            return None;
        }
        let snapshot: PollSnapshot = self
            .pending
            .iter()
            .filter(|(_, p)| p.ready && p.actions.is_empty() && p.sender.is_some())
            .map(|(key, p)| (key.clone(), p.generation.clone(), p.revision))
            .collect();
        if snapshot.is_empty() {
            return None;
        }
        self.poll_in_flight = true;
        self.next_poll = Some(now + POLL_INTERVAL);
        Some(snapshot)
    }
    fn finish_poll(&mut self, snapshot: &PollSnapshot, delivered: &HashSet<(String, String)>) {
        self.poll_in_flight = false;
        for (key, generation, revision) in snapshot {
            let Some(pending) = self.pending.get_mut(key).filter(|p| {
                p.generation == *generation
                    && p.revision == *revision
                    && p.ready
                    && p.actions.is_empty()
            }) else {
                continue;
            };
            if delivered.contains(&(key.clone(), generation.clone())) {
                pending.observed = true;
                pending.missing = 0;
            } else if pending.observed {
                // Never mistake deferred/suppressed delivery for dismissal. Two
                // fresh absent batches leave a click callback time to arrive.
                pending.missing = pending.missing.saturating_add(1);
                if pending.missing >= 2
                    && let Some(sender) = pending.sender.take()
                {
                    let _ = sender.send(Event::Dismissed);
                }
            }
        }
    }

    fn owns(&self, key: &str, generation: &str) -> bool {
        self.pending
            .get(key)
            .is_some_and(|entry| entry.generation == generation)
    }
    fn remove(&mut self, key: &str, generation: &str) {
        if self.owns(key, generation) {
            self.pending.remove(key);
        }
    }
    fn respond(&mut self, key: &str, generation: &str, action: &str) {
        let Some(entry) = self
            .pending
            .get_mut(key)
            .filter(|e| e.generation == generation)
        else {
            return;
        };
        let Some(event) = response_event(action, &entry.actions) else {
            return;
        };
        if let Some(sender) = entry.sender.take() {
            let _ = sender.send(event);
        }
    }
    fn category(&mut self, actions: Vec<(String, String)>) -> Result<String, Error> {
        // Never request relaunch-on-dismiss for a buttonless notification.
        if actions.is_empty() {
            return Ok(String::new());
        }
        if let Some(index) = self
            .categories
            .iter()
            .position(|entry| entry.actions == actions)
        {
            let entry = self.categories.remove(index);
            let id = entry.id.clone();
            self.categories.push(entry);
            return Ok(id);
        }
        if self.categories.len() == MAX_CATEGORIES {
            let index = self
                .categories
                .iter()
                .position(|category| !self.pending.values().any(|p| p.category == category.id))
                .ok_or(Error::Busy)?;
            self.categories.remove(index);
        }
        let id = format!("fastframe.category.{}", NSUUID::new().UUIDString());
        self.categories.push(Category {
            id: id.clone(),
            actions,
        });
        Ok(id)
    }
}

// Both Delivery and its response future own a guard. Dropping either removes
// only this generation, including cancellation before the future's first poll.
#[derive(Clone)]
struct Registration {
    key: String,
    generation: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        registry().remove(&self.key, &self.generation);
    }
}

fn request_id(app: &App, key: &str) -> String {
    // Length-prefixing prevents ambiguous identities; stable across restarts.
    format!("fastframe:{}:{}:{}", app.id.len(), app.id, key)
}
/// What the app asks to be allowed. The badge is asked for with alerts and
/// sounds: macOS hides the Dock tile's badge from an app registered with
/// Notification Center unless badges are allowed, and offers the "Badge
/// application icon" switch only to an app that asked for it.
fn authorization_options() -> UNAuthorizationOptions {
    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge
}
/// Whether a permission request should ask macOS again. An unanswered request
/// prompts. An answered one asks once per process without prompting, so an app
/// answered before it asked for badges gains the badge switch.
fn asks_again(request: bool, permission: Permission, asked: &AtomicBool) -> bool {
    request && (permission == Permission::NotDetermined || !asked.swap(true, Ordering::Relaxed))
}
fn permission_status(status: UNAuthorizationStatus) -> Permission {
    match status {
        UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional => {
            Permission::Granted
        }
        UNAuthorizationStatus::NotDetermined => Permission::NotDetermined,
        UNAuthorizationStatus::Denied => Permission::Denied,
        _ => Permission::Unknown,
    }
}
fn response_event(action: &str, actions: &[String]) -> Option<Event> {
    if action == unsafe { UNNotificationDefaultActionIdentifier }.to_string() {
        return Some(Event::Activated);
    }
    if action == unsafe { UNNotificationDismissActionIdentifier }.to_string() {
        return Some(Event::Dismissed);
    }
    action
        .strip_prefix(ACTION_PREFIX)
        .filter(|id| actions.iter().any(|a| a == id))
        .map(|id| Event::Action(id.to_owned()))
}
fn generation_of(request: &UNNotificationRequest) -> Option<String> {
    let info = request.content().userInfo();
    let value = info.objectForKey(&NSString::from_str(GENERATION))?;
    value.downcast_ref::<NSString>().map(ToString::to_string)
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "FastframeNativeNotificationsDelegate"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn present(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let mut options =
                UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List;
            if notification.request().content().sound().is_some() {
                options |= UNNotificationPresentationOptions::Sound;
            }
            completion.call((options,));
        }
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn respond(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let request = response.notification().request();
            if let Some(generation) = generation_of(&request) {
                registry().respond(
                    &request.identifier().to_string(),
                    &generation,
                    &response.actionIdentifier().to_string(),
                );
            }
            completion.call(());
        }
    }
);

fn center(app: &App) -> Result<Retained<UNUserNotificationCenter>, Error> {
    objc2::rc::autoreleasepool(|_| {
        mac_usernotifications::check_bundle().map_err(|_| Error::BundleRequired)?;
        if NSBundle::mainBundle()
            .bundleIdentifier()
            .as_deref()
            .map(ToString::to_string)
            .as_deref()
            != Some(app.id.as_str())
        {
            return Err(Error::InvalidInput);
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        static DELEGATE: OnceLock<Result<Retained<Delegate>, Error>> = OnceLock::new();
        let delegate = DELEGATE
            .get_or_init(|| {
                // Do not silently steal callbacks from a different notification SDK.
                if center.delegate().is_some() {
                    return Err(Error::Unsupported);
                }
                let delegate: Retained<Delegate> =
                    unsafe { objc2::msg_send![super(Delegate::alloc().set_ivars(())), init] };
                center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                Ok(delegate)
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let installed = center.delegate().ok_or(Error::Unsupported)?;
        if !std::ptr::eq(
            Retained::as_ptr(&installed).cast::<()>(),
            Retained::as_ptr(delegate).cast::<()>(),
        ) {
            return Err(Error::Unsupported);
        }
        Ok(center)
    })
}

async fn completed<T>(receiver: oneshot::Receiver<T>) -> Result<T, Error> {
    tokio::time::timeout(COMPLETION_TIMEOUT, receiver)
        .await
        .map_err(|_| Error::Delivery)?
        .map_err(|_| Error::Stopped)
}
async fn settings(center: &UNUserNotificationCenter) -> Result<Permission, Error> {
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    center.getNotificationSettingsWithCompletionHandler(&RcBlock::new(
        move |value: NonNull<UNNotificationSettings>| {
            let result = permission_status(unsafe { value.as_ref() }.authorizationStatus());
            if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = sender.send(result);
            }
        },
    ));
    completed(receiver).await
}

fn build_category(category: &Category) -> Retained<UNNotificationCategory> {
    let actions: Vec<_> = category
        .actions
        .iter()
        .map(|(id, title)| {
            UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(&format!("{ACTION_PREFIX}{id}")),
                &NSString::from_str(title),
                UNNotificationActionOptions::Foreground,
            )
        })
        .collect();
    // Only explicit named buttons get native dismissal callbacks.
    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(&category.id),
        &NSArray::from_retained_slice(&actions),
        &NSArray::new(),
        UNNotificationCategoryOptions::CustomDismissAction,
    )
}
fn publish_categories(center: &UNUserNotificationCenter, state: &Registry) {
    let built: Vec<_> = state.categories.iter().map(build_category).collect();
    center.setNotificationCategories(&NSSet::from_retained_slice(&built));
}
fn content(
    notification: &Notification,
    generation: &str,
) -> Retained<UNMutableNotificationContent> {
    objc2::rc::autoreleasepool(|_| {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        match &notification.sound {
            Sound::Default => content.setSound(Some(&UNNotificationSound::defaultSound())),
            Sound::Silent => content.setSound(None),
            Sound::Named(name) => content.setSound(Some(&UNNotificationSound::soundNamed(
                &NSString::from_str(name),
            ))),
        }
        let info = NSDictionary::from_slices(
            &[&*NSString::from_str(GENERATION)],
            &[&*NSString::from_str(generation)],
        );
        // Immutable NSString keys and values meet the property-list contract.
        unsafe { content.setUserInfo(info.cast_unchecked()) };
        content
    })
}
async fn submit(
    center: &UNUserNotificationCenter,
    registration: &Registration,
    notification: &Notification,
) -> Result<(), Error> {
    notification.validate()?;
    if settings(center).await? != Permission::Granted {
        return Err(Error::PermissionDenied);
    }
    let content = content(notification, &registration.generation);
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    objc2::rc::autoreleasepool(|_| {
        // Serialize registration, category changes and OS enqueue against close,
        // replacement and callbacks from other client workers. No lock across await.
        let mut state = registry();
        if !state.owns(&registration.key, &registration.generation) {
            return Err(Error::Stopped);
        }
        let actions = named_actions(notification);
        let category = state.category(actions.clone())?;
        if !category.is_empty() {
            content.setCategoryIdentifier(&NSString::from_str(&category));
        }
        let pending = state
            .pending
            .get_mut(&registration.key)
            .ok_or(Error::Stopped)?;
        pending.revision = pending.revision.wrapping_add(1);
        pending.ready = false;
        pending.observed = false;
        pending.missing = 0;
        pending.category = category;
        pending.actions = actions.into_iter().map(|(id, _)| id).collect();
        publish_categories(center, &state);
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&registration.key),
            &content,
            None,
        );
        center.addNotificationRequest_withCompletionHandler(
            &request,
            Some(&RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    Err(Error::Delivery)
                };
                if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let _ = sender.send(result);
                }
            })),
        );
        Ok(())
    })?;
    completed(receiver).await??;
    let mut state = registry();
    if !state.owns(&registration.key, &registration.generation) {
        return Err(Error::Stopped);
    }
    if let Some(pending) = state.pending.get_mut(&registration.key) {
        pending.ready = true;
    }
    Ok(())
}
fn remove(center: &UNUserNotificationCenter, key: &str) {
    objc2::rc::autoreleasepool(|_| {
        let ids = NSArray::from_retained_slice(&[NSString::from_str(key)]);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
    });
}

pub(crate) struct Delivery {
    center: Retained<UNUserNotificationCenter>,
    registration: Registration,
    receiver: Option<oneshot::Receiver<Event>>,
}
impl worker::Backend for Native {
    type Delivery = Delivery;
    async fn show(
        &self,
        app: &App,
        key: &str,
        notification: &Notification,
    ) -> Result<Delivery, Error> {
        let center = center(app)?;
        let key = request_id(app, key);
        let generation = objc2::rc::autoreleasepool(|_| NSUUID::new().UUIDString().to_string());
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = registry();
            if state.pending.len() >= MAX_PENDING && !state.pending.contains_key(&key) {
                return Err(Error::Busy);
            }
            state.pending.insert(
                key.clone(),
                Pending {
                    generation: generation.clone(),
                    sender: Some(sender),
                    actions: Vec::new(),
                    category: String::new(),
                    revision: 0,
                    ready: false,
                    observed: false,
                    missing: 0,
                },
            );
        }
        let delivery = Delivery {
            center,
            registration: Registration { key, generation },
            receiver: Some(receiver),
        };
        submit(&delivery.center, &delivery.registration, notification).await?;
        Ok(delivery)
    }
    async fn permission(&self, app: &App, request: bool) -> Result<Permission, Error> {
        static ASKED: AtomicBool = AtomicBool::new(false);
        let center = center(app)?;
        let permission = settings(&center).await?;
        if !asks_again(request, permission, &ASKED) {
            return Ok(permission);
        }
        if permission != Permission::NotDetermined {
            // macOS does not prompt for an answered request, but registers the
            // options it lacked; the answer itself stays the user's.
            center.requestAuthorizationWithOptions_completionHandler(
                authorization_options(),
                &RcBlock::new(|_granted: Bool, _error: *mut NSError| {}),
            );
            return Ok(permission);
        }
        let (sender, receiver) = oneshot::channel();
        let sender = Mutex::new(Some(sender));
        center.requestAuthorizationWithOptions_completionHandler(
            authorization_options(),
            &RcBlock::new(move |_granted: Bool, error: *mut NSError| {
                if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let _ = sender.send(if error.is_null() {
                        Ok(())
                    } else {
                        Err(Error::Delivery)
                    });
                }
            }),
        );
        // User interaction has no deadline. Dropping the request still releases
        // its Rust receiver; Apple owns the completion until the dialog resolves.
        receiver.await.map_err(|_| Error::Stopped)??;
        settings(&center).await
    }
    async fn withdraw(&self, app: &App, key: &str) -> Result<(), Error> {
        let center = center(app)?;
        let key = request_id(app, key);
        let mut state = registry();
        remove(&center, &key);
        if let Some(sender) = state.pending.remove(&key).and_then(|entry| entry.sender) {
            let _ = sender.send(Event::Closed);
        }
        Ok(())
    }
}
// All client workers share a single throttled OS query. The callback streams
// OS history, retaining only our bounded snapshot, never notification content.
// No detached task is spawned: dropping response futures stops future polls.
// Apple owns at most one in-flight completion, even if its service hangs.
fn poll_delivered(center: &UNUserNotificationCenter) {
    let Some(snapshot) = registry().begin_poll(Instant::now()) else {
        return;
    };
    objc2::rc::autoreleasepool(|_| {
        center.getDeliveredNotificationsWithCompletionHandler(&RcBlock::new(
            move |notifications: NonNull<NSArray<UNNotification>>| {
                objc2::rc::autoreleasepool(|_| {
                    let mut delivered = HashSet::new();
                    // SAFETY: Apple's callback supplies a live, non-null array.
                    for notification in unsafe { notifications.as_ref() } {
                        let request = notification.request();
                        let key = request.identifier().to_string();
                        if !snapshot.iter().any(|(id, _, _)| id == &key) {
                            continue;
                        }
                        if let Some(generation) = generation_of(&request)
                            && snapshot
                                .iter()
                                .any(|(id, token, _)| id == &key && token == &generation)
                        {
                            delivered.insert((key, generation));
                        }
                    }
                    registry().finish_poll(&snapshot, &delivered);
                });
            },
        ));
    });
}
// Capture the guard before the first poll, and retain it until completion. This
// is deliberately not mac-usernotifications::NotificationHandle::response().
fn response_future(
    registration: Registration,
    mut receiver: oneshot::Receiver<Event>,
    mut poll: impl FnMut() + 'static,
) -> worker::Response {
    Box::pin(async move {
        // This guard is owned even before the future is first polled.
        let _registration = registration;
        loop {
            tokio::select! {
                biased;
                result = &mut receiver => return result.unwrap_or(Event::Failed(Error::Stopped)),
                _ = tokio::time::sleep(POLL_INTERVAL) => poll(),
            }
        }
    })
}
impl worker::Delivery for Delivery {
    fn response(&mut self) -> worker::Response {
        let Some(receiver) = self.receiver.take() else {
            return Box::pin(async { Event::Failed(Error::Stopped) });
        };
        let center = self.center.clone();
        response_future(self.registration.clone(), receiver, move || {
            poll_delivered(&center)
        })
    }
    async fn update(&mut self, notification: &Notification) -> Result<(), Error> {
        submit(&self.center, &self.registration, notification).await
    }
    async fn close(&mut self) -> Result<(), Error> {
        let mut state = registry();
        if state.owns(&self.registration.key, &self.registration.generation) {
            remove(&self.center, &self.registration.key);
            state.remove(&self.registration.key, &self.registration.generation);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending(generation: &str) -> Pending {
        Pending {
            generation: generation.into(),
            sender: None,
            actions: vec!["open".into()],
            category: String::new(),
            revision: 0,
            ready: true,
            observed: false,
            missing: 0,
        }
    }
    #[test]
    fn old_guard_cannot_remove_replacement() {
        let mut state = Registry::default();
        state.pending.insert("key".into(), pending("new"));
        state.remove("key", "old");
        assert!(state.owns("key", "new"));
        state.remove("key", "new");
        assert!(state.pending.is_empty());
    }
    #[test]
    fn late_response_cannot_consume_new_sender() {
        let mut state = Registry::default();
        let (sender, mut receiver) = oneshot::channel();
        let mut entry = pending("new");
        entry.sender = Some(sender);
        state.pending.insert("key".into(), entry);
        state.respond("key", "old", "fastframe.action.open");
        assert_eq!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        state.respond("key", "new", "fastframe.action.open");
        assert_eq!(receiver.try_recv(), Ok(Event::Action("open".into())));
    }
    #[test]
    fn authorization_asks_for_the_badge_with_alerts_and_sounds() {
        let options = authorization_options();
        assert!(options.contains(UNAuthorizationOptions::Badge));
        assert!(options.contains(UNAuthorizationOptions::Alert));
        assert!(options.contains(UNAuthorizationOptions::Sound));
    }
    #[test]
    fn an_answered_permission_is_asked_again_once_per_process() {
        let asked = AtomicBool::new(false);
        assert!(!asks_again(false, Permission::Granted, &asked));
        assert!(asks_again(true, Permission::Granted, &asked));
        assert!(!asks_again(true, Permission::Granted, &asked));
        assert!(!asks_again(true, Permission::Denied, &asked));
        assert!(asks_again(true, Permission::NotDetermined, &asked));
        assert!(!asks_again(false, Permission::NotDetermined, &asked));
    }
    #[test]
    fn mappings_and_content() {
        assert_eq!(
            permission_status(UNAuthorizationStatus::NotDetermined),
            Permission::NotDetermined
        );
        assert_eq!(
            permission_status(UNAuthorizationStatus::Denied),
            Permission::Denied
        );
        assert_eq!(
            permission_status(UNAuthorizationStatus::Authorized),
            Permission::Granted
        );
        assert_eq!(
            permission_status(UNAuthorizationStatus::Provisional),
            Permission::Granted
        );
        assert_eq!(
            response_event(
                &unsafe { UNNotificationDefaultActionIdentifier }.to_string(),
                &[]
            ),
            Some(Event::Activated)
        );
        assert_eq!(
            response_event(
                &unsafe { UNNotificationDismissActionIdentifier }.to_string(),
                &[]
            ),
            Some(Event::Dismissed)
        );
        assert_eq!(
            response_event("fastframe.action.unknown", &["open".into()]),
            None
        );
        assert!(
            content(&Notification::new("Test").sound(Sound::Silent), "g")
                .sound()
                .is_none()
        );
        assert!(content(&Notification::new("Test"), "g").sound().is_some());
        assert!(
            content(
                &Notification::new("Test").sound(Sound::Named("Ping.aiff".into())),
                "g"
            )
            .sound()
            .is_some()
        );
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str("key"),
            &content(&Notification::new("Test"), "g"),
            None,
        );
        assert_eq!(generation_of(&request).as_deref(), Some("g"));
    }
    #[test]
    fn request_keys_are_stable_and_unambiguous() {
        let app = App::new("com.test.app", "Test");
        assert_eq!(request_id(&app, "key"), request_id(&app, "key"));
        assert_ne!(
            request_id(&app, "a:b"),
            request_id(&App::new("com.test.app.a", "Test"), "b")
        );
    }
    #[test]
    fn categories_reuse_and_bound_inactive_history() {
        let mut state = Registry::default();
        let first = state
            .category(vec![("open".into(), "Open".into())])
            .unwrap();
        assert_eq!(
            state
                .category(vec![("open".into(), "Open".into())])
                .unwrap(),
            first
        );
        for i in 0..MAX_CATEGORIES + 10 {
            state
                .category(vec![("open".into(), format!("Open {i}"))])
                .unwrap();
        }
        assert_eq!(state.categories.len(), MAX_CATEGORIES);
    }
    #[test]
    fn cancellation_before_first_poll_unregisters() {
        let key = "test-cancel-unpolled".to_owned();
        registry().pending.insert(key.clone(), pending("g"));
        let guard = Registration {
            key: key.clone(),
            generation: "g".into(),
        };
        let (_sender, receiver) = oneshot::channel();
        let future = response_future(guard, receiver, || {});
        drop(future);
        assert!(!registry().pending.contains_key(&key));
    }
    #[tokio::test(flavor = "current_thread")]
    async fn polled_response_cancellation_drops_sender_and_preserves_replacement() {
        use std::task::{Context, Waker};
        let key = "test-cancel-polled".to_owned();
        let (sender, receiver) = oneshot::channel();
        let mut entry = pending("old");
        entry.sender = Some(sender);
        registry().pending.insert(key.clone(), entry);
        let mut future = response_future(
            Registration {
                key: key.clone(),
                generation: "old".into(),
            },
            receiver,
            || {},
        );
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        drop(future);
        assert!(!registry().pending.contains_key(&key));
        let (sender, receiver) = oneshot::channel();
        let mut entry = pending("old");
        entry.sender = Some(sender);
        registry().pending.insert(key.clone(), entry);
        let future = response_future(
            Registration {
                key: key.clone(),
                generation: "old".into(),
            },
            receiver,
            || {},
        );
        registry().pending.insert(key.clone(), pending("new"));
        drop(future);
        assert!(registry().owns(&key, "new"));
        registry().remove(&key, "new");
    }
    #[test]
    fn category_capacity_preserves_every_active_category() {
        let mut state = Registry::default();
        for i in 0..MAX_CATEGORIES {
            let category = state
                .category(vec![("open".into(), format!("Open {i}"))])
                .unwrap();
            let mut entry = pending("g");
            entry.category = category;
            state.pending.insert(i.to_string(), entry);
        }
        assert_eq!(
            state.category(vec![("other".into(), "Other".into())]),
            Err(Error::Busy)
        );
        assert_eq!(state.categories.len(), MAX_CATEGORIES);
        state.pending.remove("0");
        assert!(
            state
                .category(vec![("other".into(), "Other".into())])
                .is_ok()
        );
        assert_eq!(state.categories.len(), MAX_CATEGORIES);
    }
    #[test]
    fn buttonless_and_default_only_never_register_a_category() {
        let mut state = Registry::default();
        for notification in [
            Notification::new("Test"),
            Notification::new("Test").action("default", "Open"),
        ] {
            assert!(named_actions(&notification).is_empty());
            assert_eq!(state.category(named_actions(&notification)).unwrap(), "");
            assert!(state.categories.is_empty());
            assert!(content(&notification, "g").categoryIdentifier().is_empty());
        }
        let notification = Notification::new("Test")
            .action("default", "Open")
            .action("details", "Details");
        let actions = named_actions(&notification);
        assert_eq!(actions, vec![("details".into(), "Details".into())]);
        let category = build_category(&Category {
            id: "test".into(),
            actions,
        });
        assert!(
            category
                .options()
                .contains(UNNotificationCategoryOptions::CustomDismissAction)
        );
        assert_eq!(category.actions().len(), 1);
        assert_eq!(
            category.actions().objectAtIndex(0).identifier().to_string(),
            "fastframe.action.details"
        );
    }
    fn polling_entry(
        state: &mut Registry,
        key: &str,
        generation: &str,
    ) -> oneshot::Receiver<Event> {
        let (sender, receiver) = oneshot::channel();
        let mut entry = pending(generation);
        entry.actions.clear();
        entry.sender = Some(sender);
        state.pending.insert(key.into(), entry);
        receiver
    }
    #[test]
    fn polls_are_batched_throttled_and_single_flight() {
        let mut state = Registry::default();
        let _a = polling_entry(&mut state, "a", "g");
        let _b = polling_entry(&mut state, "b", "g");
        let _named = polling_entry(&mut state, "named", "g");
        state
            .pending
            .get_mut("named")
            .unwrap()
            .actions
            .push("open".into());
        let _unready = polling_entry(&mut state, "unready", "g");
        state.pending.get_mut("unready").unwrap().ready = false;
        let now = Instant::now();
        let snapshot = state.begin_poll(now).unwrap();
        assert_eq!(snapshot.len(), 2);
        assert!(state.begin_poll(now + Duration::from_secs(60)).is_none());
        state.finish_poll(&snapshot, &HashSet::new());
        assert!(state.begin_poll(now).is_none());
        assert!(state.begin_poll(now + POLL_INTERVAL).is_some());
    }
    #[test]
    fn delayed_delivery_is_not_dismissal_and_two_absent_batches_are_required() {
        let mut state = Registry::default();
        let mut receiver = polling_entry(&mut state, "key", "g");
        let snapshot = state.begin_poll(Instant::now()).unwrap();
        for _ in 0..4 {
            state.finish_poll(&snapshot, &HashSet::new());
        }
        assert_eq!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        state.finish_poll(&snapshot, &HashSet::from([("key".into(), "g".into())]));
        state.finish_poll(&snapshot, &HashSet::new());
        assert_eq!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        state.finish_poll(&snapshot, &HashSet::new());
        assert_eq!(receiver.try_recv(), Ok(Event::Dismissed));
    }
    #[test]
    fn stale_poll_cannot_dismiss_updated_replaced_or_cancelled_delivery() {
        let mut state = Registry::default();
        let mut receiver = polling_entry(&mut state, "key", "g");
        let snapshot = state.begin_poll(Instant::now()).unwrap();
        state.pending.get_mut("key").unwrap().observed = true;
        state.pending.get_mut("key").unwrap().revision += 1;
        for _ in 0..3 {
            state.finish_poll(&snapshot, &HashSet::new());
        }
        assert_eq!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        let mut replacement = polling_entry(&mut state, "key", "new");
        state.pending.get_mut("key").unwrap().observed = true;
        for _ in 0..3 {
            state.finish_poll(&snapshot, &HashSet::new());
        }
        assert_eq!(
            replacement.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        state.remove("key", "new");
        state.finish_poll(&snapshot, &HashSet::new());
        assert!(state.pending.is_empty());
        assert!(state.begin_poll(Instant::now() + POLL_INTERVAL).is_none());
    }
    #[test]
    fn click_takes_priority_during_dismissal_grace() {
        let mut state = Registry::default();
        let mut receiver = polling_entry(&mut state, "key", "g");
        let snapshot = state.begin_poll(Instant::now()).unwrap();
        state.finish_poll(&snapshot, &HashSet::from([("key".into(), "g".into())]));
        state.finish_poll(&snapshot, &HashSet::new());
        state.respond(
            "key",
            "g",
            &unsafe { UNNotificationDefaultActionIdentifier }.to_string(),
        );
        state.finish_poll(&snapshot, &HashSet::new());
        assert_eq!(receiver.try_recv(), Ok(Event::Activated));
    }
}

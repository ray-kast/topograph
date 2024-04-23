use std::{
    cell::OnceCell,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc, Weak,
    },
    task::{Context, Poll, Waker},
};

use crossbeam::queue::SegQueue;

#[derive(Debug)]
pub struct Mutex<T: ?Sized>(PhantomData<&'static T>);
pub struct MutexGuard<'a, T: ?Sized>(PhantomData<&'a mut T>);

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self { Self(PhantomData) }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T>> { async move { MutexGuard(todo!()) } }
}

impl<'a, T> std::ops::Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target { todo!() }
}

impl<'a, T> std::ops::DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { todo!() }
}

pub struct Unpark(AtomicPtr<Waker>);

impl Unpark {
    const NEW: Self = Self::new();
    const UNPARKED: *mut Waker = NonNull::dangling().as_ptr();
    const UNPOLLED: *mut Waker = ptr::null_mut();

    #[inline]
    const fn new() -> Self { Self(AtomicPtr::new(Self::UNPOLLED)) }

    unsafe fn get_waker(ptr: *mut Waker) -> Option<Box<Waker>> {
        match ptr {
            Self::UNPOLLED | Self::UNPARKED => None,
            w => Some(Box::from_raw(w)),
        }
    }

    fn box_waker(w: Box<Waker>) -> *mut Waker { Box::into_raw(w) }

    #[inline]
    pub fn unpark(&self) {
        let ptr = self.0.swap(Self::UNPARKED, Ordering::AcqRel);
        if let Some(waker) = unsafe { Self::get_waker(ptr) } {
            waker.wake();
        }
    }
}

impl Drop for Unpark {
    fn drop(&mut self) {
        let ptr = self.0.swap(Self::UNPARKED, Ordering::AcqRel);
        if let Some(waker) = unsafe { Self::get_waker(ptr) } {
            drop(waker);
        }
    }
}

pub struct Park(Arc<Unpark>);

impl Future for Park {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let waker = OnceCell::new();
        let prev = self
            .0
            .0
            .fetch_update(Ordering::Release, Ordering::Acquire, |p| {
                (p == Unpark::UNPOLLED)
                    .then(|| *waker.get_or_init(|| Unpark::box_waker(cx.waker().clone().into())))
            });
        match prev {
            Ok(p) => {
                debug_assert_eq!(p, Unpark::UNPOLLED);
                Poll::Pending
            },
            Err(Unpark::UNPARKED) => Poll::Ready(()),
            Err(p) => {
                unreachable!("Invalid unpark pointer {p:x?} encountered, this should not happen")
            },
        }
    }
}

pub fn park() -> (Park, Weak<Unpark>) {
    let unpark = Arc::new(Unpark::NEW);
    let unpark_weak = Arc::downgrade(&unpark);

    (Park(unpark), unpark_weak)
}

#[derive(Debug)]
pub struct Condvar(SegQueue<Weak<Unpark>>);

impl Condvar {
    #[inline]
    pub const fn new() -> Self { Self(SegQueue::new()) }

    pub fn wait<T>(&self, guard: &mut MutexGuard<'_, T>) -> Park {
        let (park, unpark) = park();
        self.0.push(unpark);
        park
    }

    pub fn notify_one(&self) {
        loop {
            let Some(unpark) = self.0.pop() else { break };
            let Some(unpark) = unpark.upgrade() else {
                continue;
            };
            unpark.unpark();
            break;
        }
    }

    pub fn notify_all(&self) {
        for _ in 0..self.0.len() {
            let Some(unpark) = self.0.pop() else { break };
            let Some(unpark) = unpark.upgrade() else {
                continue;
            };
            unpark.unpark();
            break;
        }
    }
}
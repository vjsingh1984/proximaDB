//! Safe wrappers around `Box::into_raw` / `Box::from_raw` for the JNI
//! handle pattern.
//!
//! Spark JNI `createPartitionReader` / `createDataWriter` return a
//! `jlong` opaque handle that Java treats as an integer. Internally
//! we leak a `Box<T>` and stash its raw pointer in the handle; later
//! `readNextBatch` / `writeBatch` / `closePartitionReader` reconstitute
//! the `Box` via [`take`] (consuming the handle) or borrow the
//! contents via [`borrow_mut`] (preserving the handle for further
//! calls).
//!
//! All `unsafe` related to this pattern lives in this one auditable
//! module; callers stay safe. Invariants every helper enforces:
//!
//! * A handle is valid for exactly one call to [`take`] (consumes it).
//! * [`borrow_mut`] returns `None` for a null handle (jlong == 0) and
//!   yields a `&'static mut T` for non-null — caller MUST NOT call
//!   [`take`] on the same handle while a borrow is live.
//! * Two threads MUST NOT call [`borrow_mut`] on the same handle
//!   concurrently. JNI callers respect this by virtue of Spark task
//!   isolation (one task per reader/writer); the unit-test surface
//!   never hands handles across threads.
//!
//! Note: `T` must be `Send + 'static` so the Box can be moved across
//! the FFI boundary. The Spark partition reader / data writer state
//! structs satisfy this.

use jni::sys::jlong;

/// Leak a `Box<T>` and return its raw pointer as a `jlong`. The
/// returned handle is opaque to Java; pair with [`take`] to reclaim.
pub fn leak<T: Send + 'static>(value: T) -> jlong {
    Box::into_raw(Box::new(value)) as jlong
}

/// Consume the handle and return the boxed value back to the caller.
/// `None` when the handle is null (jlong == 0) — JNI callers rely on
/// this so a double-close call returns safely instead of segfaulting.
///
/// # Safety
///
/// Caller MUST guarantee the handle was originally produced by
/// [`leak`] (same `T`) and has not been [`take`]n or actively
/// [`borrow_mut`]'d elsewhere.
pub unsafe fn take<T: Send + 'static>(handle: jlong) -> Option<Box<T>> {
    if handle == 0 {
        return None;
    }
    // SAFETY: caller-asserted invariants (see above).
    Some(unsafe { Box::from_raw(handle as *mut T) })
}

/// Borrow a mutable reference to the boxed value without consuming
/// the handle. `None` when the handle is null. Used by
/// `readNextBatch` / `writeBatch` which call repeatedly on the same
/// handle.
///
/// # Safety
///
/// Caller MUST guarantee:
/// * the handle was originally produced by [`leak`] (same `T`),
/// * no concurrent [`take`] or [`borrow_mut`] for the same handle is
///   active on another thread.
pub unsafe fn borrow_mut<T: Send + 'static>(handle: jlong) -> Option<&'static mut T> {
    if handle == 0 {
        return None;
    }
    // SAFETY: caller-asserted invariants (see above). The 'static
    // lifetime is correct because the value was leaked by `leak` and
    // lives until `take` reclaims it.
    Some(unsafe { &mut *(handle as *mut T) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leak_take_round_trip() {
        let h = leak::<String>("hello".to_string());
        assert_ne!(h, 0);
        let boxed = unsafe { take::<String>(h) }.expect("not null");
        assert_eq!(*boxed, "hello");
    }

    #[test]
    fn test_take_null_handle_returns_none() {
        let opt = unsafe { take::<String>(0) };
        assert!(opt.is_none());
    }

    #[test]
    fn test_borrow_mut_lets_caller_mutate_in_place() {
        let h = leak::<Vec<i32>>(vec![1, 2, 3]);
        {
            let borrowed = unsafe { borrow_mut::<Vec<i32>>(h) }.expect("not null");
            borrowed.push(4);
        }
        // Reclaim with take to confirm the mutation stuck.
        let final_value = unsafe { take::<Vec<i32>>(h) }.expect("not null");
        assert_eq!(*final_value, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_borrow_mut_null_handle_returns_none() {
        let opt: Option<&mut String> = unsafe { borrow_mut::<String>(0) };
        assert!(opt.is_none());
    }
}

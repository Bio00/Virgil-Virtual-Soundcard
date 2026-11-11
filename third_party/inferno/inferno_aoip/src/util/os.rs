use thread_priority::Error;

/// Virgil patch: a real-time thread for work that repeats every `period_ns`.
/// On macOS the POSIX priorities this crate maps to are refused
/// ("PriorityNotInRange(15..=47)"), so the network threads ran at normal
/// priority; they get the Mach time-constraint policy Core Audio threads and
/// Virgil's own engine use instead. Elsewhere this is set_current_thread_realtime.
pub fn set_current_thread_realtime_period(priority_hint: u8, period_ns: u64) -> Result<(), Error> {
  #[cfg(target_os = "macos")]
  {
    let _ = priority_hint;
    return mac_rt::set_time_constraint(period_ns)
      .map_err(|code| Error::OS(code));
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = period_ns;
    set_current_thread_realtime(priority_hint)
  }
}

#[cfg(target_os = "macos")]
mod mac_rt {
  #[repr(C)]
  struct TimeConstraintPolicy {
    period: u32,
    computation: u32,
    constraint: u32,
    preemptible: i32,
  }
  #[repr(C)]
  #[derive(Default)]
  struct TimebaseInfo {
    numer: u32,
    denom: u32,
  }
  extern "C" {
    fn mach_thread_self() -> u32;
    fn thread_policy_set(thread: u32, flavor: u32, policy_info: *const i32, count: u32) -> i32;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    static mach_task_self_: u32;
  }
  const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
  const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;

  /// Parameters as Virgil's engine uses them (src/common/platform.cpp):
  /// computation a quarter of the period but at least 100 us (the kernel's
  /// minimum quantum is 50 us), constraint half the period, never below the
  /// computation. Returns the kernel's error code on failure.
  pub fn set_time_constraint(period_ns: u64) -> Result<(), i32> {
    let period_ns = period_ns.clamp(100_000, 50_000_000);
    let computation = (period_ns / 4).max(100_000);
    let constraint = (period_ns / 2).max(computation);
    let mut tb = TimebaseInfo::default();
    unsafe {
      if mach_timebase_info(&mut tb) != 0 || tb.numer == 0 {
        return Err(-1);
      }
    }
    let to_abs = |ns: u64| (ns as u128 * tb.denom as u128 / tb.numer as u128).min(u32::MAX as u128) as u32;
    let pol = TimeConstraintPolicy {
      period: to_abs(period_ns),
      computation: to_abs(computation),
      constraint: to_abs(constraint),
      preemptible: 1,
    };
    unsafe {
      let thread = mach_thread_self();
      let r = thread_policy_set(
        thread,
        THREAD_TIME_CONSTRAINT_POLICY,
        &pol as *const TimeConstraintPolicy as *const i32,
        THREAD_TIME_CONSTRAINT_POLICY_COUNT,
      );
      mach_port_deallocate(mach_task_self_, thread);
      if r == 0 {
        Ok(())
      } else {
        Err(r)
      }
    }
  }

  #[cfg(test)]
  mod tests {
    #[test]
    fn time_constraint_is_accepted() {
      // A plain user thread may take the policy (no root needed).
      std::thread::spawn(|| assert_eq!(super::set_time_constraint(333_000), Ok(()))).join().unwrap();
    }
  }
}

#[cfg(target_family = "unix")]
pub fn set_current_thread_realtime(priority_hint: u8) -> Result<(), Error> {
  use thread_priority::thread_native_id;
  use thread_priority::unix::set_thread_priority_and_policy;
  use thread_priority::{
    RealtimeThreadSchedulePolicy, ThreadPriority, ThreadPriorityValue, ThreadSchedulePolicy,
  };

  set_thread_priority_and_policy(
    thread_native_id(),
    ThreadPriority::Crossplatform(ThreadPriorityValue::try_from(priority_hint).unwrap()),
    ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
  )
}

#[cfg(windows)]
#[link(name = "avrt")]
extern "system" {
  fn AvSetMmThreadCharacteristicsW(task_name: *const u16, task_index: *mut u32) -> isize;
  fn AvSetMmThreadPriority(handle: isize, priority: i32) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
  fn GetCurrentThread() -> isize;
  fn SetThreadPriority(thread: isize, priority: i32) -> i32;
  fn GetLastError() -> u32;
}

/// Virgil patch. On Windows the thread joins the MMCSS "Pro Audio" class at
/// AVRT_PRIORITY_CRITICAL, like the audio engine, and its priority is then
/// left to MMCSS (a SetThreadPriority afterwards would replace the MMCSS
/// real-time boost with THREAD_PRIORITY_HIGHEST). Without MMCSS (service
/// stopped) it falls back to THREAD_PRIORITY_TIME_CRITICAL and returns an
/// error saying so, for the caller to log; the MMCSS outcome is logged here.
#[cfg(not(target_family = "unix"))]
pub fn set_current_thread_realtime(_priority_hint: u8) -> Result<(), Error> {
  #[cfg(windows)]
  unsafe {
    const AVRT_PRIORITY_CRITICAL: i32 = 2;
    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
    let task: Vec<u16> = "Pro Audio".encode_utf16().chain(std::iter::once(0)).collect();
    let mut index = 0u32;
    let h = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index);
    if h != 0 {
      if AvSetMmThreadPriority(h, AVRT_PRIORITY_CRITICAL) == 0 {
        log::warn!(
          "thread {:?} joined MMCSS \"Pro Audio\" but could not set critical priority (error {})",
          std::thread::current().name(),
          GetLastError()
        );
      } else {
        log::info!(
          "thread {:?} runs in MMCSS \"Pro Audio\" at critical priority",
          std::thread::current().name()
        );
      }
      return Ok(());
    }
    let mmcss_error = GetLastError();
    if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) != 0 {
      log::debug!("AvSetMmThreadCharacteristicsW failed (error {mmcss_error})");
      return Err(Error::Ffi("MMCSS unavailable, using THREAD_PRIORITY_TIME_CRITICAL instead"));
    }
    return Err(Error::OS(GetLastError() as i32));
  }
  #[cfg(not(windows))]
  thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max)
}

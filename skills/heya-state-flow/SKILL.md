---
name: heya-state-flow
description: Simplify Heya's asynchronous UI and backend state flows when fixing silent actions, duplicate submissions, stale results, or lifecycle races. Apply to stateful controls and their Rust workers, rather than visual-only changes.
---

# Heya state flow

Trace the actual action through the Slint callback, command channel, worker,
protocol/archive result and UI publication before changing it. Read the relevant
parts of `docs/architecture.md` and keep application types independent of Slint
and the protocol SDK.

- Keep durable content, capabilities and operation results owned by the backend.
  The UI may own input, presentation and a small pending gate after submission is
  accepted. Do not synthesize a second backend status just to show progress.
- Validate the action, reserve or enqueue its command, then perform associated
  UI changes. Use the existing bounded channel; when preparatory side effects
  must precede dispatch, reserve capacity first. Rejected commands must not stop
  unrelated playback, clear input, or imply that work started.
- Show local checking separately from network progress. Missing prerequisites and
  queue/service failures need visible feedback; an enabled control must not
  silently do nothing. Backend checks still decide whether work can proceed.
- Preserve existing request/revision matching across navigation and delayed
  completions. A coalesced snapshot carries current state, not a reliable queue
  of one-shot events. Retain acknowledgements long enough for the UI to consume
  them. Do not add sequence numbers where existing identity/order is sufficient.
- Every accepted submission must reach a result, including disconnection before
  the worker starts. Keep drafts/selections on failure; clear only the input
  actually acknowledged, preserving edits made while awaiting the result.
- Reuse existing cancellation and shutdown paths. Prioritize stopping when
  needed, release protocol/device ownership, and join finite media workers.
  Do not add idle threads, polling timers, generic dispatch layers or retry
  frameworks to repair a local ordering defect.
- Put device deadlines outside blocking platform calls, covering discovery and
  opening too. Require actual media progress rather than a worker heartbeat.
  Use the existing finite-helper pattern when a plugin cannot be interrupted;
  close its pipe and reap it before publishing completion or starting a replacement.

Reproduce the observable failure with synthetic services, then make the smallest
fix or remove the conflicting state. Test delayed responses, double clicks,
queue rejection, navigation/disconnect during work and cancellation where they
affect that flow. Exercise the real callback/worker boundary where practical.
Use Heya's offscreen scripts for native screenshots and performance checks;
never send real messages or place real calls automatically. Check that progress
updates reuse timeline models and that idle/closed media leaves no workers.

Apply this review to the affected paths, preserving correctly ordered flows.
Describe what was changed and what the tests establish; synthetic success does
not establish device, account or macOS interoperability.

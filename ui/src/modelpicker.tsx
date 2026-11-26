/**
 * Backwards-compatible entry point.
 *
 * Item 134 moved the picker to the roadmap's `model-picker.tsx` path. Session, Settings and the
 * agent roster still import `ModelControls` from here, so this module keeps the full public API by
 * delegating to the canonical implementation. New code should import from `./model-picker`.
 */

export * from "./model-picker";

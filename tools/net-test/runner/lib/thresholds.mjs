// Every numeric threshold of track N in one place (net-test NT-D9). The values come from
// the acceptance criteria; a scenario reads them from here and cannot loosen them. `provisional` marks the
// preliminary ones.

const t = (value, ac, ref, provisional = false) => Object.freeze({ value, ac, ref, provisional });

export const THRESHOLDS = Object.freeze({
  "latency.p50_over_rtt_ms": t(60, "AC-N2.1", "BUILD N2.1", true),
  "latency.p95_over_rtt_ms": t(150, "AC-N2.1", "BUILD N2.1", true),
  "optimistic.max_frames": t(1, "AC-N2.2", "BUILD N2.2"),
  "prefetch.queries": t(0, "AC-N2.3", "BUILD N2.3"),
  "prefetch.first_select_over_rtt_ms": t(50, "AC-N2.3", "BUILD N2.3"),
  "step.p95_ms.duel4": t(20, "AC-N2.5", "BUILD N2.5", true),
  "step.p95_ms.huge8": t(60, "AC-N2.5", "BUILD N2.5", true),
  "weak.p95_ms": t(1000, "AC-N3.1", "BUILD N3.1"),
  "weak.false_drops": t(0, "AC-N3.1", "BUILD N3.1"),
  "outage.cut_s": t(10, "AC-N3.2", "BUILD N3.2"),
  "outage.recover_ms": t(2000, "AC-N3.2", "BUILD N3.2"),
  // The numbers of the client's probing policy (net-client 4.7); change only together with it.
  "outage.probe_start_ms": t(4000, "AC-N3.2", "net-client 4.7"),
  "outage.probe_every_ms": t(500, "AC-N3.2", "net-client 4.7"),
  "outage.probe_attempt_ms": t(1000, "AC-N3.2", "net-client 4.7"),
  "outage.probe_max_live": t(2, "AC-N3.2", "net-client 4.7"),
  "reload.ms": t(5000, "AC-N3.3", "BUILD N3.3"),
  "background.minutes": t(10, "AC-N3.4", "BUILD N3.4"),
  "background.recover_ms": t(2000, "AC-N3.4", "BUILD N3.4"),
  // Keyframe sizes are without the map basis (NP-D22), in bytes on the wire (compressed, the gateway's
  // `tx.keyframe_bytes_max`; decision 2026-10-22); the raw size is printed as information.
  "traffic.duel.kb_turn": t(50, "AC-N3.5", "BUILD N3.5", true),
  "traffic.duel.keyframe_kb": t(15, "AC-N3.5", "BUILD N3.5", true),
  "traffic.huge.kb_turn": t(200, "AC-N3.5", "BUILD N3.5", true),
  "traffic.huge.keyframe_kb": t(80, "AC-N3.5", "BUILD N3.5", true),
  "traffic.up_kb_turn": t(5, "AC-N3.5", "BUILD N3.5", true),
  "join.total_ms": t(10000, "AC-N4.5", "BUILD N4.5"),
  "join.others_stall_ms": t(1000, "AC-N4.5", "BUILD N4.5"),
  "kill9.recover_s": t(30, "AC-N5.2", "BUILD N5.2"),
  "kill9.lost_orders_max_s": t(60, "AC-N5.2", "BUILD N5.2"),
  "ai.duel.median_s": t(0.3, "AC-N5.4", "BUILD N5.4", true),
  "ai.std8.median_s": t(2, "AC-N5.4", "BUILD N5.4", true),
  "ai.std8.max_s": t(5, "AC-N5.4", "BUILD N5.4", true),
  "ai.huge.median_s": t(5, "AC-N5.4", "BUILD N5.4", true),
  "soak.bots": t(6, "AC-N6.1", "BUILD N6.1"),
  "soak.turns": t(200, "AC-N6.1", "BUILD N6.1"),
  "host.handover_s": t("AC-N4.6"),
  "host.handover_tolerance_s": t("AC-N4.6"),
  "hash.mismatch": t(0, "AC-N1.2", "BUILD N1.2"),
  "hash.heal_over_rtt_ms": t(60, "AC-N1.2", "BUILD N1.2"),
});

/** The threshold record of `key`; an unknown key is a bug of the scenario. */
export function th(key) {
  const r = THRESHOLDS[key];
  if (!r) throw new Error(`unknown threshold ${key} (see runner/lib/thresholds.mjs)`);
  return r;
}

/** The value of `key`. */
export const thv = (key) => th(key).value;

/** `60 (AC-N2.1, prov.)`: how a threshold is printed next to a measurement. */
export function thText(key, unit = "") {
  const r = th(key);
  return `${r.value}${unit} (${r.ac}${r.provisional ? ", prov." : ""})`;
}

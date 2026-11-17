// Revision cache contract for a hover transport supplied by the host.
export function richTooltipCache(query, now = () => Date.now()) {
  let epoch = 0, revision, seat, wanted, pending = null, lastAt = -Infinity;
  const values = new Map();
  return async function hover(player, rev, kind, id, city = null) {
    if (player !== seat || rev !== revision) {
      epoch++; seat = player; revision = rev; values.clear(); pending = null; lastAt = -Infinity;
    }
    const key = JSON.stringify([kind, id, city]);
    wanted = key;
    if (values.has(key)) return values.get(key);
    if (pending) return pending.key === key ? pending.promise : null;
    if (now() - lastAt < 250) return null;
    lastAt = now();
    const generation = epoch;
    const promise = Promise.resolve().then(() => query(player, {kind, id, ...(city === null ? {} : {city})})).then(value => {
      if (generation !== epoch) return null;
      if (values.size >= 64) values.delete(values.keys().next().value);
      values.set(key, value); return wanted === key ? value : null;
    }).finally(() => { if (generation === epoch) pending = null; });
    pending = {key, promise};
    return promise;
  };
}

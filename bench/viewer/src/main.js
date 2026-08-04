// nd-image-codecs bench viewer — scaffold.
//
// Reads BenchRecord JSON (see bench/rs/ndic-bench-core) from the records
// directory this site is generated next to, and renders per-benchmark history
// and config overlays. The full feature set (ref lanes, rate–distortion) is
// not built yet.

export async function loadRecords(indexUrl = "./records/index.json") {
  const res = await fetch(indexUrl);
  if (!res.ok) throw new Error(`failed to load ${indexUrl}: ${res.status}`);
  return res.json();
}

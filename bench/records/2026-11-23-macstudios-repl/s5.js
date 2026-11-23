const M = input.members;            // [{id, ip}] x3, node n+1 / rqlite node n
const C = input.client;             // runner id of the client Mac
const D = "~/.cache/amphora-s5", B = "~/.cache/amphora-s5-bin/amphora-bench";
const peers = M.map(m => `${m.ip}:${input.aport}`).join(",");
const FF = "--batch 1 --text-bytes 600 --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5";
const log = [];
const host = async (tag) => {
  const cmd = `echo "$(hostname) $(date -u +%FT%TZ) | $(uptime | sed 's/.*load averages*: //') | free_mb $(( $(vm_stat | awk '/Pages free/{print $3}' | tr -d .) * 16 / 1024 )) inactive_mb $(( $(vm_stat | awk '/Pages inactive/{print $3}' | tr -d .) * 16 / 1024 )) | $(memory_pressure -Q 2>/dev/null | tail -1)"`;
  const ids = [...M.map(m => m.id), C];
  const r = await Promise.all(ids.map(on => kiln.machines.run({on, sync: false, command: cmd}).then(x => x.output.trim()).catch(e => `${on} ERR ${e}`)));
  log.push(`${tag}: ` + r.join(" ;; "));
};
const settle = async (ids) => {
  const out = [];
  for (const id of ids) {
    let o = await kiln.machines.output({id, wait_s: 20}).catch(e => ({output: String(e)}));
    if (o.running !== false) { await kiln.machines.stop({id}).catch(() => {}); o = await kiln.machines.output({id, wait_s: 10}).catch(e => ({output: String(e)})); }
    out.push({exit: o.exit_code, tail: (o.output || "").slice(-300)});
  }
  return out;
};
const pair = async (round, s, w) => {
  const R = `${D}/rec/round${round}`;
  let starts, client;
  if (s.startsWith("amphora")) {
    const cls = s.split("-")[1];
    starts = await Promise.all(M.map((m, n) => kiln.machines.start({on: m.id, sync: false,
      command: `rm -rf ${D}/m; mkdir -p ${D}/m; exec ${D}/bin/amphora-bench --repl-member --node ${n + 1} --peers ${peers} --dir ${D}/m/${n + 1} --class ${cls} --spanning 2 --relay-records 128 --deadline-s 10`})));
    client = await kiln.machines.run({on: C, sync: false, timeout_s: 120,
      command: `mkdir -p ${R}; ${B} --repl commit --system amphora-procs --class ${cls} --members ${peers} --record ${R}/amphora-procs-${cls}-commit.jsonl ${FF} --writers ${w} 2>&1 | tail -5`});
  } else {
    const join = M.map(m => `${m.ip}:${input.rport + 10}`).join(",");
    starts = await Promise.all(M.map((m, n) => kiln.machines.start({on: m.id, sync: false,
      command: `rm -rf ${D}/rq; mkdir -p ${D}/rq; exec ${D}/bin/rqlited -node-id ${n} -http-addr ${m.ip}:${input.rport} -raft-addr ${m.ip}:${input.rport + 10} -bootstrap-expect 3 -join ${join} ${D}/rq/data`})));
    client = await kiln.machines.run({on: C, sync: false, timeout_s: 240,
      command: `mkdir -p ${R}; RQ_IPS="${M.map(m => m.ip).join(" ")}" RQ_PORT=${input.rport} ${B} --repl commit --system rqlite --scripts ${D}/scripts --shell /bin/bash --record ${R}/rqlite-commit.jsonl ${FF} --writers ${w} 2>&1 | tail -5`});
    for (const st of starts) await kiln.machines.stop({id: st.id}).catch(() => {});
  }
  const members = await settle(starts.map(x => x.id));
  log.push(`round ${round} ${s} w${w}: client exit ${client.exit_code} ${client.output.trim().slice(-400)} | members ${JSON.stringify(members.map(m => m.exit))}`);
  if (client.exit_code !== 0) log.push(`  member tails: ${JSON.stringify(members.map(m => m.tail))}`);
};
const A = ["amphora-persisted", "amphora-replicated", "rqlite"];
for (const round of input.rounds) {
  const order = round % 2 ? A : [...A].reverse();
  await host(`round ${round} start (${order.join(" ")})`);
  for (const w of input.ladder) for (const s of order) await pair(round, s, w);
  await host(`round ${round} end`);
}
await kiln.machines.run({on: C, sync: false, command: `mkdir -p ${D}/rec; cat >> ${D}/rec/rounds.txt <<'LOGEOF'\n${log.join("\n")}\nLOGEOF`});
return log;

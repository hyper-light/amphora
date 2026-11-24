# Lane B25: takes one part of the fix out of the tree it stands in (never for main).
import sys
MUTS={
 'm1':('a short restore leaves its files','crates/amphora/src/backup/restore.rs',
  "        Err(e) if matches!(e.downcast_ref(), Some(Unrestorable::Short { .. })) => e,\n",
  "        Err(e) if matches!(e.downcast_ref(), Some(Unrestorable::Short { archived: 0, .. })) => e,\n"),
 'm2':('what was replayed is not compared','crates/amphora/src/backup/restore.rs',
  "    if replayed != frames {\n        return Ok(replayed);\n    }\n", "    let replayed = frames.max(replayed);\n"),
 'm3':('end_base takes a base begun before the break','crates/amphora/src/backup/ship.rs',
  "        if self.broke_since(&copy) {\n            anyhow::bail!(", "        if self.broke_since(&copy) && copy.next == u64::MAX {\n            anyhow::bail!("),
 'm4':('the round copies on a base the log broke under','crates/amphora/src/backup/shipping.rs',
  "        if copy.as_ref().is_some_and(|c| self.shipper.broke_since(c)) {\n            *copy = None;\n        }\n",""),
 'm5':('an opening counts a base from its end','crates/amphora/src/backup/ship.rs',
  "            shipper.base_start = Some(manifest.start_gen);\n","            shipper.base_start = Some(manifest.end.0);\n"),
 'm6':('an opening on the closed segment counts its frames as the current one\'s','crates/amphora/src/backup/ship.rs',
  "            Some(i) if i.seq == seq => shipper.shipped,\n            _ => 0,\n","            Some(_) => shipper.shipped,\n            None => 0,\n"),
 'm7':('a new database keeps the second segment left there','crates/engine/database.rs',
  "        if frames == 0 || self.db_file.size()? > 0 {\n            return Ok(segment);\n        }\n","        if frames < u64::MAX || self.db_file.size()? > 0 {\n            return Ok(segment);\n        }\n"),
 'm8':('a forced generation is gapped whatever was shipped','crates/amphora/src/backup.rs',
  "            return Ok(frame == e.frame && (!e.forced || shipped));\n","            return Ok(frame == e.frame && !e.forced && (shipped || frame == e.frame));\n"),
 'm9':('a fence is not told where a log is opened','crates/amphora/src/backup/ship.rs',
  "                    shipper.hooks.fence.opened(seq, frame)?;\n","                    let _ = (seq, frame);\n"),
 'm10':('a fence told of a log opened takes it over its own writer\'s restarts','crates/amphora/src/backup.rs',
  "        if state.ended.is_none() && state.ending.is_none() {\n            state.ended = Some(Ended {","        if state.ending.is_none() {\n            state.ended = Some(Ended {"),
}
tag=sys.argv[1]
what,path,old,new=MUTS[tag]
s=open(path).read()
assert s.count(old)==1,(tag,s.count(old))
open(path,'w').write(s.replace(old,new))
print(tag,what)

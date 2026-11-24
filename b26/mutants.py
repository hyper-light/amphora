# Lane B26's mutants: each part of the fix taken out of the tree it runs in. usage: mutants.py NAME
import sys
R='crates/amphora/src/backup/restore.rs'; S='crates/amphora/src/backup/ship.rs'; T='crates/amphora/src/backup/shipping.rs'
D='crates/amphora/src/db.rs'; C='crates/amphora/src/backup/cut.rs'
def cut_block():
    s=open(C).read(); a=s.index('    let mut made = Vec::with_capacity'); b=s.index('    let mut databases')
    return [(C, s[a:b], '''    let mut restored = Vec::with_capacity(groups.len());
    for (prefix, path) in groups {
        let mut pace = Unpaced;
        let (making, group) = restore_aside(blobs, prefix, second, path, &mut pace, &memory, until)?;
        making.end()?;
        restored.push(group);
    }
    let _: fn(&std::path::Path) -> anyhow::Result<()> = remove;
''')]
M={
 'm1-manifest-not-compared':lambda:[(R,'    if let Some(lack) = manifest.held.lacks(began.and_then(|s| s.link)) {\n        return Err(unproven(lack).into());\n    }\n','    let _ = began;\n')],
 'm2-own-generation-not-compared':lambda:[(R,'if generation == manifest.start_gen && archived != manifest.held.own {','if generation == u64::MAX && archived != manifest.held.own {')],
 'm3-begun-before-the-segment-is-in-the-file':lambda:[(S,'        if !db.closing_copied() {\n            anyhow::bail!','        if db.path().as_os_str().is_empty() {\n            anyhow::bail!'),(S,'        if let Some(lack) = held.lacks(self.began) {','        if let Some(lack) = held.lacks(self.began).filter(|_| false) {')],
 'm4-a-failed-restores-files-kept':lambda:[(R,'        if let Err(e) = remove(&self.file) {','        if let Err(e) = remove(std::path::Path::new("/nowhere/at/all")) {')],
 'm5-made-at-its-own-path':lambda:[(R,'        let file = restoring_path(path);\n        for path in','        let file = if path.as_os_str().is_empty() { restoring_path(path) } else { path.to_path_buf() };\n        for path in'),(R,'    pub(super) fn end(self) -> anyhow::Result<()> {\n','    pub(super) fn end(self) -> anyhow::Result<()> {\n        if self.file == self.path {\n            std::mem::forget(self);\n            return Ok(());\n        }\n')],
 'm6-its-path-not-refused':lambda:[(D,'    if making.exists() && !path.exists() {','    if making.as_os_str().is_empty() && !path.exists() {')],
 'm7-its-name-not-refused':lambda:[(D,'    if restoring {\n','    if restoring && path.as_os_str().is_empty() {\n')],
 'm8-a-failed-check-asks-nothing':lambda:[(T,'            self.mends.ask(*lacks);','            let _ = lacks;')],
 'm9-the-ask-wakes-nothing':lambda:[(T,'            let _pending = wake.try_send(());','            let _ = wake;')],
 'm10-the-log-not-shipped-again':lambda:[(S,'                    self.begin_again();','                    let _: fn(&mut Self) = Self::begin_again;')],
 'm11-each-group-put-at-its-path-as-it-is-made':cut_block,
 'm12-a-killed-checks-files-kept':lambda:[(T,'    remove(&restoring_path(scratch))\n','    remove(&restoring_path(std::path::Path::new("/nowhere/at/all")))\n')],
 'm13-no-bound-on-the-asks':lambda:[(S,'        if !needed || self.mended == Some(end) {','        if !needed || (self.mended == Some(end) && end.1 == u64::MAX) {')],
 'm14-the-base-for-it-keeps-chunks':lambda:[(S,'        self.last_base = None;\n        self.traced_from = None;\n        self.mended = Some(end);','        self.mended = Some(end);')],
}
if sys.argv[1]=='list':
    print(' '.join(M))
else:
    for f,old,new in M[sys.argv[1]]():
        s=open(f).read(); assert s.count(old)==1,(f,s.count(old),old); open(f,'w').write(s.replace(old,new,1))

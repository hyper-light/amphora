import gdb, os
out = open(os.environ["PMP_OUT"], "w")
gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("set print thread-events off")
gdb.execute("handle SIGINT stop print nopass")
gdb.execute("handle SIGPIPE nostop noprint pass")
def sample():
    inf = gdb.selected_inferior()
    for t in inf.threads():
        try:
            t.switch()
            names = []
            fr = gdb.newest_frame()
            while fr is not None and len(names) < 80:
                names.append(fr.name() or "??")
                fr = fr.older()
            out.write("T%d|%s|%s\n" % (t.num, t.name or "", ";".join(reversed(names))))
        except gdb.error as e:
            out.write("T%d|err|%s\n" % (t.num, e))
    out.write("--\n")
    out.flush()
try:
    gdb.execute("run", to_string=True)
except gdb.error:
    pass
while True:
    inf = gdb.selected_inferior()
    if not inf.pid or not inf.threads():
        break
    sample()
    try:
        gdb.execute("continue", to_string=True)
    except gdb.error:
        break
out.close()

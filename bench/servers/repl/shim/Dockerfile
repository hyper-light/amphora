# The bench's shim (main.go) with dqlite's C library and LiteFS beside it: one image for
# servers/repl/dqlite.sh and servers/repl/litefs.sh.
FROM ubuntu:24.04 AS build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends golang-go gcc libc6-dev libdqlite-dev libsqlite3-dev ca-certificates git pkg-config
WORKDIR /src
COPY main.go .
RUN go mod init shim && go get github.com/canonical/go-dqlite@latest github.com/mattn/go-sqlite3@latest && go mod tidy \
  && CGO_LDFLAGS_ALLOW="-Wl,-z,now" go build -tags libsqlite3 -o /shim .

FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends libdqlite-dev libsqlite3-0 fuse3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=flyio/litefs:0.5 /usr/local/bin/litefs /usr/local/bin/litefs
COPY --from=build /shim /usr/local/bin/shim
COPY litefs.yml /etc/litefs.yml

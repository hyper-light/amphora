// The bench's shim for the replicated SQLites without an HTTP API of their own (bench/src/repl.rs):
// dqlite through go-dqlite's app package, and SQLite on a LiteFS mount. It takes
//
//	POST /exec  {"stmts": [[sql, arg...], ...]}  the statements in one transaction: {} or {"error": ...}
//	POST /count {"stmts": [[sql, arg...]]}       the first column of the first row: {"count": n}
//	GET  /ready                                  200 once the database takes statements
//	GET  /leader                                 dqlite's leader address; on LiteFS the lease holder's
//	                                             hostname: this node's when it takes a write
//	                                             transaction, else its .primary file's
//	GET  /voters                                 dqlite: {"voters": n, "members": m}, as the leader's
//	                                             configuration has them
//
// Once it listens and its database takes statements, it prints "shim ready: listening on ADDR" on
// stderr: the ready signal ../lib.sh waits for.
package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"flag"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/canonical/go-dqlite/app"
	_ "github.com/mattn/go-sqlite3"
)

type request struct {
	Stmts [][]any `json:"stmts"`
}

func args(stmt []any) (string, []any, bool) {
	if len(stmt) == 0 {
		return "", nil, false
	}
	q, ok := stmt[0].(string)
	out := make([]any, 0, len(stmt)-1)
	for _, a := range stmt[1:] {
		if n, isNum := a.(json.Number); isNum {
			i, err := n.Int64()
			if err != nil {
				return "", nil, false
			}
			out = append(out, i)
		} else {
			out = append(out, a)
		}
	}
	return q, out, ok
}

func decode(r *http.Request) (request, error) {
	var req request
	d := json.NewDecoder(r.Body)
	d.UseNumber()
	err := d.Decode(&req)
	return req, err
}

func reply(w http.ResponseWriter, v any) {
	w.Header().Set("content-type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

func main() {
	backend := flag.String("backend", "dqlite", "dqlite or sqlite")
	dir := flag.String("dir", "", "dqlite's data directory")
	address := flag.String("address", "", "dqlite's address, ip:port (it binds what it advertises)")
	join := flag.String("join", "", "dqlite: comma-separated addresses to join")
	path := flag.String("path", "", "sqlite: the database file")
	listen := flag.String("listen", "", "HTTP address, ip:port")
	// The name /leader gives for this node on LiteFS (its lease hostname): nodes sharing one host
	// each name themselves (../litefs.sh).
	name := flag.String("name", "", "sqlite: this node's lease hostname")
	flag.Parse()
	ctx := context.Background()
	required := map[string][]*string{"dqlite": {dir, address, listen}, "sqlite": {path, name, listen}}
	for _, v := range required[*backend] {
		if *v == "" {
			log.Fatalf("the %s backend takes -dir, -address and -listen (dqlite) or -path, -name and -listen (sqlite)", *backend)
		}
	}

	var db *sql.DB
	var dq *app.App
	var err error
	switch *backend {
	case "dqlite":
		if err = os.MkdirAll(*dir, 0o755); err != nil {
			log.Fatal(err)
		}
		opts := []app.Option{app.WithAddress(*address)}
		if *join != "" {
			opts = append(opts, app.WithCluster(strings.Split(*join, ",")))
		}
		if dq, err = app.New(*dir, opts...); err != nil {
			log.Fatal(err)
		}
		if err = dq.Ready(ctx); err != nil {
			log.Fatal(err)
		}
		if db, err = dq.Open(ctx, "bench"); err != nil {
			log.Fatal(err)
		}
	case "sqlite":
		// The WAL journal LiteFS replicates; synchronous=FULL so a commit is acknowledged durable;
		// every transaction takes the write lock at BEGIN, which only the primary grants.
		if db, err = sql.Open("sqlite3", "file:"+*path+"?_journal_mode=WAL&_synchronous=FULL&_busy_timeout=5000&_txlock=immediate"); err != nil {
			log.Fatal(err)
		}
		db.SetMaxOpenConns(1)
	default:
		log.Fatalf("unknown backend %s", *backend)
	}

	http.HandleFunc("/ready", func(w http.ResponseWriter, r *http.Request) {
		// LiteFS starts the shim once its node holds the lease or follows its holder, and a
		// replica's database file doesn't exist until the primary's first write: only dqlite's
		// database is pinged.
		if dq == nil {
			reply(w, map[string]any{})
			return
		}
		if err := db.PingContext(r.Context()); err != nil {
			http.Error(w, err.Error(), http.StatusServiceUnavailable)
			return
		}
		reply(w, map[string]any{})
	})
	http.HandleFunc("/leader", func(w http.ResponseWriter, r *http.Request) {
		if dq == nil {
			if tx, err := db.BeginTx(r.Context(), nil); err == nil {
				_ = tx.Rollback()
				_, _ = w.Write([]byte(*name))
				return
			}
			primary, err := os.ReadFile(filepath.Join(filepath.Dir(*path), ".primary"))
			if err != nil {
				http.Error(w, "no primary", http.StatusServiceUnavailable)
				return
			}
			_, _ = w.Write([]byte(strings.TrimSpace(string(primary))))
			return
		}
		cli, err := dq.Leader(r.Context())
		if err != nil {
			http.Error(w, err.Error(), http.StatusServiceUnavailable)
			return
		}
		defer cli.Close()
		info, err := cli.Leader(r.Context())
		if err != nil || info == nil {
			http.Error(w, "no leader", http.StatusServiceUnavailable)
			return
		}
		_, _ = w.Write([]byte(info.Address))
	})
	// go-dqlite's app joins a node as a spare and promotes voters only once three nodes are in
	// the cluster (app/roles.go, RolesChanges.Assume): a node can be Ready before it votes. The
	// cluster script reads this once every node is ready, so a commit's quorum is known, not assumed.
	http.HandleFunc("/voters", func(w http.ResponseWriter, r *http.Request) {
		if dq == nil {
			http.Error(w, "dqlite only", http.StatusNotFound)
			return
		}
		cli, err := dq.Leader(r.Context())
		if err != nil {
			http.Error(w, err.Error(), http.StatusServiceUnavailable)
			return
		}
		defer cli.Close()
		nodes, err := cli.Cluster(r.Context())
		if err != nil {
			http.Error(w, err.Error(), http.StatusServiceUnavailable)
			return
		}
		voters := 0
		for _, n := range nodes {
			if n.Role.String() == "voter" {
				voters++
			}
		}
		reply(w, map[string]any{"voters": voters, "members": len(nodes)})
	})
	http.HandleFunc("/exec", func(w http.ResponseWriter, r *http.Request) {
		req, err := decode(r)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		// One statement is its own transaction, as rqlite's and sqld's one-statement requests are:
		// one exchange with the database, not BEGIN, the statement and COMMIT (three of dqlite's
		// round trips to its leader).
		if len(req.Stmts) == 1 {
			q, a, ok := args(req.Stmts[0])
			if !ok {
				http.Error(w, "a statement is [sql, arg...]", http.StatusBadRequest)
				return
			}
			if _, err := db.ExecContext(r.Context(), q, a...); err != nil {
				reply(w, map[string]any{"error": err.Error()})
				return
			}
			reply(w, map[string]any{})
			return
		}
		tx, err := db.BeginTx(r.Context(), nil)
		if err != nil {
			reply(w, map[string]any{"error": err.Error()})
			return
		}
		for _, s := range req.Stmts {
			q, a, ok := args(s)
			if !ok {
				_ = tx.Rollback()
				http.Error(w, "a statement is [sql, arg...]", http.StatusBadRequest)
				return
			}
			if _, err := tx.ExecContext(r.Context(), q, a...); err != nil {
				_ = tx.Rollback()
				reply(w, map[string]any{"error": err.Error()})
				return
			}
		}
		if err := tx.Commit(); err != nil {
			reply(w, map[string]any{"error": err.Error()})
			return
		}
		reply(w, map[string]any{})
	})
	http.HandleFunc("/count", func(w http.ResponseWriter, r *http.Request) {
		req, err := decode(r)
		if err != nil || len(req.Stmts) != 1 {
			http.Error(w, "one statement", http.StatusBadRequest)
			return
		}
		q, a, ok := args(req.Stmts[0])
		if !ok {
			http.Error(w, "a statement is [sql, arg...]", http.StatusBadRequest)
			return
		}
		var n int64
		if err := db.QueryRowContext(r.Context(), q, a...).Scan(&n); err != nil {
			http.Error(w, err.Error(), http.StatusServiceUnavailable)
			return
		}
		reply(w, map[string]any{"count": n})
	})
	// Bound before the ready line, so a client that read the line connects at once. The line is
	// the shim's own ready signal (../lib.sh waits for it on the node's log): dqlite's node is
	// Ready and its database open, or LiteFS has made this node primary or connected it to one
	// (LiteFS runs the shim only then). It goes to stderr, which LiteFS passes on as its own.
	ln, err := net.Listen("tcp", *listen)
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("shim ready: listening on %s", ln.Addr())
	log.Fatal(http.Serve(ln, nil))
}

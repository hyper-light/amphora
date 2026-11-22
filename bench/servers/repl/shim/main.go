// The bench's shim for the replicated SQLites without an HTTP API of their own (bench/src/repl.rs):
// dqlite through go-dqlite's app package, and SQLite on a LiteFS mount. It takes
//   POST /exec  {"stmts": [[sql, arg...], ...]}  the statements in one transaction: {} or {"error": ...}
//   POST /count {"stmts": [[sql, arg...]]}       the first column of the first row: {"count": n}
//   GET  /ready                                  200 once the database takes statements
//   GET  /leader                                 dqlite's leader address; on LiteFS the lease holder's
//                                                hostname: this node's when it takes a write
//                                                transaction, else its .primary file's
package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"flag"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/canonical/go-dqlite/v2/app"
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
	dir := flag.String("dir", "/data", "dqlite's data directory")
	address := flag.String("address", "", "dqlite's address, host:port")
	join := flag.String("join", "", "dqlite: comma-separated addresses to join")
	path := flag.String("path", "/litefs/bench.db", "sqlite: the database file")
	listen := flag.String("listen", ":8080", "HTTP address")
	flag.Parse()
	ctx := context.Background()

	var db *sql.DB
	var dq *app.App
	var err error
	switch *backend {
	case "dqlite":
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
				host, err := os.Hostname()
				if err != nil {
					http.Error(w, err.Error(), http.StatusServiceUnavailable)
					return
				}
				_, _ = w.Write([]byte(host))
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
	http.HandleFunc("/exec", func(w http.ResponseWriter, r *http.Request) {
		req, err := decode(r)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
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
	log.Fatal(http.ListenAndServe(*listen, nil))
}

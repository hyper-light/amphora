/*
 * The parser oracle of docs/design/postgres.md section 3 (unit 3 of section 11): PostgreSQL 19's own
 * raw_parser, unpatched, linked from scripts/postgres-oracle-extract.sh's extraction of REL_19_0.
 * Test-only, run as a subprocess by crates/pg-oracle so the Rust side has no `unsafe`.
 *
 * stdin: statements, each ended by a NUL byte. stdout, one frame a statement:
 *   T <bytes>\n<nodeToStringWithLocations of raw_parser's List of RawStmt>\n
 *   E <SQLSTATE> <cursor> <bytes>\n<message>\n
 * The cursor is the error's character position, 1-based, 0 for none (ErrorResponse's P field).
 * With `--tokens` a T frame's body is instead the tokens raw_parser's grammar reads (base_yylex,
 * parser.c's lookahead filter over the scanner), a line each: `<token> <byte offset> <bytes> <value>`,
 * the token gram.h's number, the value its string, integer or keyword (empty for the others).
 */
#include "postgres.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mb/pg_wchar.h"
#include "miscadmin.h"
#include "nodes/nodes.h"
#include "parser/parser.h"
#include "common/keywords.h"
#include "gramparse.h"
#include "utils/guc_hooks.h"
#include "utils/memutils.h"

static void
frame(char kind, const char *head, const char *body)
{
	size_t		len = strlen(body);

	printf("%c %s%s%zu\n", kind, head, head[0] ? " " : "", len);
	fwrite(body, 1, len, stdout);
	putchar('\n');
}

static char *
tokens(const char *sql)
{
	base_yy_extra_type yyextra;
	core_yyscan_t yyscanner = scanner_init(sql, &yyextra.core_yy_extra, &ScanKeywords, ScanKeywordTokens);
	StringInfoData out;
	YYSTYPE		lval;
	YYLTYPE		loc;
	int			tok;
	char		num[16];

	yyextra.have_lookahead = false;
	initStringInfo(&out);
	while ((tok = base_yylex(&lval, &loc, yyscanner)) != 0)
	{
		const char *v = "";

		switch (tok)
		{
			case IDENT:
			case UIDENT:
			case FCONST:
			case SCONST:
			case USCONST:
			case BCONST:
			case XCONST:
			case Op:
				v = lval.str;
				break;
			case ICONST:
			case PARAM:
				snprintf(num, sizeof(num), "%d", lval.ival);
				v = num;
				break;
			default:
				/* gram.y declares the keywords, then the filter's _LA tokens, in one run. */
				if (tok >= ABORT_P && tok <= WITHOUT_LA)
					v = lval.keyword;
				break;
		}
		appendStringInfo(&out, "%d %d %zu %s\n", tok, (int) loc, strlen(v), v);
	}
	scanner_finish(yyscanner);
	return out.data;
}

static void
parse_one(const char *sql, bool scan)
{
	MemoryContext stmt = AllocSetContextCreate(TopMemoryContext, "oracle statement", ALLOCSET_DEFAULT_SIZES);
	MemoryContext old = MemoryContextSwitchTo(stmt);

	PG_TRY();
	{
		frame('T', "", scan ? tokens(sql) : nodeToStringWithLocations(raw_parser(sql, RAW_PARSE_DEFAULT)));
	}
	PG_CATCH();
	{
		ErrorData  *e;
		char		head[32];
		char		state[6];
		int			code;

		MemoryContextSwitchTo(stmt);
		e = CopyErrorData();
		FlushErrorState();
		code = e->sqlerrcode;
		for (int i = 0; i < 5; i++)
		{
			state[i] = PGUNSIXBIT(code);
			code >>= 6;
		}
		state[5] = '\0';
		snprintf(head, sizeof(head), "%s %d", state, e->cursorpos);
		frame('E', head, e->message ? e->message : "");
	}
	PG_END_TRY();

	MemoryContextSwitchTo(old);
	MemoryContextDelete(stmt);
}

int
main(int argc, char **argv)
{
	bool		scan = argc == 2 && strcmp(argv[1], "--tokens") == 0;
	size_t		cap = 1 << 16;
	size_t		len = 0;
	char	   *buf = malloc(cap);
	int			c;

	MemoryContextInit();
	SetDatabaseEncoding(PG_UTF8);
	/* As the server sets it under an 8 MB stack rlimit: guc.c InitializeGUCOptions, 2048 kB. */
	set_stack_base();
	assign_max_stack_depth(2048, NULL);

	if (buf == NULL)
		return 1;
	while ((c = getchar()) != EOF)
	{
		if (len + 1 == cap)
		{
			cap *= 2;
			buf = realloc(buf, cap);
			if (buf == NULL)
				return 1;
		}
		buf[len++] = (char) c;
		if (c != '\0')
			continue;
		parse_one(buf, scan);
		fflush(stdout);
		len = 0;
	}
	return 0;
}

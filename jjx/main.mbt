///|
/// jjx - JJ + Git workflow wrapper

let program_name = "jjx"

struct CmdResult {
  exit_code : Int
  stdout : String
  stderr : String
}

async fn main raise {
  let args = @sys.get_cli_args()
  if args.length() <= 1 {
    print_usage()
    return
  }

  let command = args[1]
  match command {
    "init" => init_repo()
    "status" | "st" => run_jj(["st"])
    "fetch" => run_jj_safe(["git", "fetch"])
    "rebase" => rebase_command(args)
    "push" => push_command(args)
    "sync" => sync_command(args)
    "help" | "-h" | "--help" => print_usage()
    _ => {
      println("Unknown command: " + command)
      print_usage()
    }
  }
}

fn print_usage() -> Unit {
  println(program_name + " - JJ + Git workflow wrapper")
  println("")
  println("USAGE:")
  println("  " + program_name + " <command>")
  println("")
  println("COMMANDS:")
  println("  init            Initialize jj (colocated with git when possible)")
  println("  status, st      jj status")
  println("  fetch           jj git fetch")
  println("  rebase [dest]   jj rebase -d <dest> (default: main)")
  println("  push <bm>       jj git push --bookmark <bm>")
  println("  push --all      jj git push --all")
  println("  sync <bm>       fetch + rebase + push <bm>")
}

async fn init_repo() -> Unit raise {
  let cwd = get_cwd()
  let git_dir = cwd + "/.git"
  let has_git = is_dir(git_dir)
  if has_git {
    run_jj_safe(["git", "init", "--colocate"])
  } else {
    run_jj_safe(["git", "init"])
  }
}

async fn rebase_command(args : Array[String]) -> Unit raise {
  let dest = if args.length() > 2 { args[2] } else { "main" }
  run_jj_safe(["rebase", "-d", dest])
}

async fn push_command(args : Array[String]) -> Unit raise {
  if args.length() > 2 && args[2] == "--all" {
    run_jj_safe(["git", "push", "--all"])
    return
  }
  if args.length() <= 2 {
    println("Missing bookmark name")
    return
  }
  run_jj_safe(["git", "push", "--bookmark", args[2]])
}

async fn sync_command(args : Array[String]) -> Unit raise {
  if args.length() <= 2 {
    println("Missing bookmark name")
    return
  }
  run_jj_safe(["git", "fetch"])
  run_jj_safe(["rebase", "-d", "main"])
  run_jj_safe(["git", "push", "--bookmark", args[2]])
}

async fn run_jj(args : Array[String]) -> Unit raise {
  let cwd = get_cwd()
  let result = run_exec("jj", args, cwd)
  if result.stdout.length() > 0 {
    println(result.stdout)
  }
  if result.exit_code != 0 {
    if result.stderr.length() > 0 { println(result.stderr) }
  }
}

async fn run_jj_safe(args : Array[String]) -> Unit raise {
  if !ensure_git_ready() {
    return
  }
  run_jj(args)
}

async fn run_exec(command : String, args : Array[String], cwd : String) -> CmdResult raise {
  let (exit_code, stdout_data, stderr_data) = @process.collect_output(
    "sh",
    ["-c", command + " " + shell_join(args)],
    cwd~,
  )
  CmdResult::{ exit_code, stdout: stdout_data.text(), stderr: stderr_data.text() }
}

fn shell_join(parts : Array[String]) -> String {
  let escaped : Array[String] = []
  for part in parts {
    escaped.push("'" + escape_single_quotes(part) + "'")
  }
  escaped.join(" ")
}

fn escape_single_quotes(s : String) -> String {
  s.replace(old="'", new="'\\''")
}

fn get_cwd() -> String {
  match @sys.get_env_var("PWD") {
    Some(path) => path
    None => "."
  }
}

fn is_dir(path : String) -> Bool {
  try { @fs.is_dir(path) } catch { _ => false }
}

async fn ensure_git_ready() -> Bool raise {
  let cwd = get_cwd()
  let git_dir = resolve_git_dir(cwd)
  if git_dir.length() == 0 {
    return true
  }
  let busy =
    path_exists(git_dir + "/rebase-merge") ||
    path_exists(git_dir + "/rebase-apply") ||
    path_exists(git_dir + "/MERGE_HEAD") ||
    path_exists(git_dir + "/CHERRY_PICK_HEAD") ||
    path_exists(git_dir + "/REVERT_HEAD") ||
    path_exists(git_dir + "/BISECT_LOG")
  if busy {
    println("Git operation in progress. Resolve it before running jjx.")
    return false
  }
  true
}

async fn resolve_git_dir(cwd : String) -> String raise {
  let result = run_exec("git", ["rev-parse", "--git-dir"], cwd)
  if result.exit_code != 0 {
    return ""
  }
  let raw = result.stdout.trim(chars=" \n\t\r")
  if raw.length() == 0 {
    return ""
  }
  if raw.has_prefix("/") {
    raw.to_string()
  } else {
    cwd + "/" + raw.to_string()
  }
}

fn path_exists(path : String) -> Bool {
  @fs.path_exists(path)
}

// Onclick handler example — updates a var via meh2 IPC.
// Used as: :onclick "scripts/greet.rhai"
// The return value is executed as a shell command.
let name = run_shell("whoami");
`meh2 update GREET_MSG=Hello, ${name}!`

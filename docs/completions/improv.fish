# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_improv_global_optspecs
    string join \n h/help V/version
end

function __fish_improv_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_improv_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_improv_using_subcommand
    set -l cmd (__fish_improv_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c improv -n "__fish_improv_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_needs_command" -s V -l version -d 'Print version'
complete -c improv -n "__fish_improv_needs_command" -f -a "init" -d 'Create or open a store and install the schema'
complete -c improv -n "__fish_improv_needs_command" -f -a "add-category" -d 'Add a category (a dimension, such as Time)'
complete -c improv -n "__fish_improv_needs_command" -f -a "add-item" -d 'Add an item (a member of a category)'
complete -c improv -n "__fish_improv_needs_command" -f -a "add-measure" -d 'Add an input measure'
complete -c improv -n "__fish_improv_needs_command" -f -a "add-derived" -d 'Add a derived (formula) measure'
complete -c improv -n "__fish_improv_needs_command" -f -a "define" -d 'Create a measure or LAMBDA from a definition, "Target = ..."'
complete -c improv -n "__fish_improv_needs_command" -f -a "register-ext" -d 'Register a Python external function'
complete -c improv -n "__fish_improv_needs_command" -f -a "refresh-ext" -d 'Run a CALL(...) measure\'s function and store its cells'
complete -c improv -n "__fish_improv_needs_command" -f -a "set" -d 'Set an input cell'
complete -c improv -n "__fish_improv_needs_command" -f -a "list" -d 'Print the categories, items, measures, views and input cells'
complete -c improv -n "__fish_improv_needs_command" -f -a "save-view" -d 'Save a named view of a measure, as the GUI does'
complete -c improv -n "__fish_improv_needs_command" -f -a "format" -d 'Set how a measure\'s numbers are shown'
complete -c improv -n "__fish_improv_needs_command" -f -a "show" -d 'Print one measure and its input cells'
complete -c improv -n "__fish_improv_needs_command" -f -a "eval" -d 'Compute a derived measure and print its cells'
complete -c improv -n "__fish_improv_needs_command" -f -a "scenario" -d 'Add an input-cell override to a what-if scenario'
complete -c improv -n "__fish_improv_needs_command" -f -a "export" -d 'Print the whole model as JSON'
complete -c improv -n "__fish_improv_needs_command" -f -a "import-sql" -d 'Import a SQLite query into a new SQL-backed input measure'
complete -c improv -n "__fish_improv_needs_command" -f -a "refresh-sql" -d 'Re-run an SQL-backed measure\'s query and replace its cells'
complete -c improv -n "__fish_improv_needs_command" -f -a "refresh-all" -d 'Refresh every external measure now'
complete -c improv -n "__fish_improv_needs_command" -f -a "serve-refresh" -d 'Refresh measures as their refresh policies fall due, until interrupted'
complete -c improv -n "__fish_improv_needs_command" -f -a "export-sql" -d 'Write a measure\'s input cells to a SQLite table'
complete -c improv -n "__fish_improv_needs_command" -f -a "import-csv" -d 'Import a CSV or TSV file into a new input measure'
complete -c improv -n "__fish_improv_needs_command" -f -a "export-csv" -d 'Write a measure\'s cells to a CSV or TSV file'
complete -c improv -n "__fish_improv_needs_command" -f -a "stream" -d 'Apply input edits from stdin and print the cells they change'
complete -c improv -n "__fish_improv_needs_command" -f -a "policy" -d 'Manage a model\'s access policies (Cedar)'
complete -c improv -n "__fish_improv_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c improv -n "__fish_improv_using_subcommand init" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand add-category" -s h -l help -d 'Print help'
complete -c improv -n "__fish_improv_using_subcommand add-item" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand add-measure" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand add-derived" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand define" -l refresh -d 'A CALL(...) measure\'s refresh policy' -r
complete -c improv -n "__fish_improv_using_subcommand define" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand register-ext" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand refresh-ext" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand set" -l at -d 'The cell\'s coordinate: Category=Item pairs, comma-joined, space-separated, or in repeated --at flags' -r
complete -c improv -n "__fish_improv_using_subcommand set" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand list" -s h -l help -d 'Print help'
complete -c improv -n "__fish_improv_using_subcommand save-view" -l rows -d 'The row categories, comma-separated, outer first' -r
complete -c improv -n "__fish_improv_using_subcommand save-view" -l cols -d 'The column categories, comma-separated, outer first' -r
complete -c improv -n "__fish_improv_using_subcommand save-view" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand format" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand show" -s h -l help -d 'Print help'
complete -c improv -n "__fish_improv_using_subcommand eval" -l scenario -d 'The scenario to evaluate under' -r
complete -c improv -n "__fish_improv_using_subcommand eval" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand scenario" -l at -d 'The cell\'s coordinate: Category=Item pairs, comma-joined, space-separated, or in repeated --at flags' -r
complete -c improv -n "__fish_improv_using_subcommand scenario" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand export" -s h -l help -d 'Print help'
complete -c improv -n "__fish_improv_using_subcommand import-sql" -l refresh -d 'The measure\'s refresh policy' -r
complete -c improv -n "__fish_improv_using_subcommand import-sql" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand refresh-sql" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand refresh-all" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand serve-refresh" -l tick -d 'Seconds between ticks' -r
complete -c improv -n "__fish_improv_using_subcommand serve-refresh" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand export-sql" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand import-csv" -l tsv -d 'Tab-separated'
complete -c improv -n "__fish_improv_using_subcommand import-csv" -l no-header -d 'The file has no header row: columns are 0-based indices'
complete -c improv -n "__fish_improv_using_subcommand import-csv" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand export-csv" -l tsv -d 'Tab-separated'
complete -c improv -n "__fish_improv_using_subcommand export-csv" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand stream" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "validate" -d 'Check a policy file against the schema and the model, storing nothing'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "set" -d 'Validate a policy file and store it with the model'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "show" -d 'Print the stored policies, or say there are none'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "clear" -d 'Remove the stored policies: the model is unrestricted again'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "check" -d 'Ask whether a user may do an action, and say which policies decided'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "template" -d 'Print a template\'s policies, to edit or set'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "lint" -d 'List what a user can infer of what is hidden from them'
complete -c improv -n "__fish_improv_using_subcommand policy; and not __fish_seen_subcommand_from validate set show clear check template lint help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from validate" -l groups -d 'The groups, a JSON file: {"group": {"users": [...], "groups": [...]}}' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from validate" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from set" -l groups -d 'The groups, a JSON file' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from set" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from show" -l groups -d 'Print the groups JSON'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from clear" -s h -l help -d 'Print help'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l user -d 'The user asking' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l role -d 'A role the user has (repeatable)' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l measure -d 'A measure: its numeric id, or its name (all digits is an id)' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l item -d 'An item, as Category=Item' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l category -d 'A category, by name' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l view -d 'A view, by name' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l at -d 'The cell\'s coordinate: Category=Item pairs, comma-joined, space-separated, or in repeated --at flags' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l policies -d 'A policy file to check instead of the stored policies' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -l groups -d 'The groups for --policies, a JSON file' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from check" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from template" -l user -d 'The user it grants to' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from template" -l group -d 'The group it grants to' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from template" -l measure -d 'A measure (editor, hide-measures; repeatable)' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from template" -l item -d 'An item as Category=Item (analyst; repeatable)' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from template" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from lint" -l user -d 'The user' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from lint" -l role -d 'A role the user has (repeatable)' -r
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from lint" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "validate" -d 'Check a policy file against the schema and the model, storing nothing'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "set" -d 'Validate a policy file and store it with the model'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "show" -d 'Print the stored policies, or say there are none'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "clear" -d 'Remove the stored policies: the model is unrestricted again'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "check" -d 'Ask whether a user may do an action, and say which policies decided'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "template" -d 'Print a template\'s policies, to edit or set'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "lint" -d 'List what a user can infer of what is hidden from them'
complete -c improv -n "__fish_improv_using_subcommand policy; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "init" -d 'Create or open a store and install the schema'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "add-category" -d 'Add a category (a dimension, such as Time)'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "add-item" -d 'Add an item (a member of a category)'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "add-measure" -d 'Add an input measure'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "add-derived" -d 'Add a derived (formula) measure'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "define" -d 'Create a measure or LAMBDA from a definition, "Target = ..."'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "register-ext" -d 'Register a Python external function'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "refresh-ext" -d 'Run a CALL(...) measure\'s function and store its cells'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "set" -d 'Set an input cell'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "list" -d 'Print the categories, items, measures, views and input cells'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "save-view" -d 'Save a named view of a measure, as the GUI does'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "format" -d 'Set how a measure\'s numbers are shown'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "show" -d 'Print one measure and its input cells'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "eval" -d 'Compute a derived measure and print its cells'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "scenario" -d 'Add an input-cell override to a what-if scenario'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "export" -d 'Print the whole model as JSON'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "import-sql" -d 'Import a SQLite query into a new SQL-backed input measure'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "refresh-sql" -d 'Re-run an SQL-backed measure\'s query and replace its cells'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "refresh-all" -d 'Refresh every external measure now'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "serve-refresh" -d 'Refresh measures as their refresh policies fall due, until interrupted'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "export-sql" -d 'Write a measure\'s input cells to a SQLite table'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "import-csv" -d 'Import a CSV or TSV file into a new input measure'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "export-csv" -d 'Write a measure\'s cells to a CSV or TSV file'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "stream" -d 'Apply input edits from stdin and print the cells they change'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "policy" -d 'Manage a model\'s access policies (Cedar)'
complete -c improv -n "__fish_improv_using_subcommand help; and not __fish_seen_subcommand_from init add-category add-item add-measure add-derived define register-ext refresh-ext set list save-view format show eval scenario export import-sql refresh-sql refresh-all serve-refresh export-sql import-csv export-csv stream policy help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "validate" -d 'Check a policy file against the schema and the model, storing nothing'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "set" -d 'Validate a policy file and store it with the model'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "show" -d 'Print the stored policies, or say there are none'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "clear" -d 'Remove the stored policies: the model is unrestricted again'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "check" -d 'Ask whether a user may do an action, and say which policies decided'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "template" -d 'Print a template\'s policies, to edit or set'
complete -c improv -n "__fish_improv_using_subcommand help; and __fish_seen_subcommand_from policy" -f -a "lint" -d 'List what a user can infer of what is hidden from them'

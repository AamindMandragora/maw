let
  nixpkgs = import ./nixpkgs-lib;
  inherit (nixpkgs)
    concatLists
    concatMapStrings
    concatMapStringsSep
    concatStrings
    concatStringsSep
    filterAttrs
    hasInfix
    hasPrefix
    isAttrs
    isList
    mapAttrsToList
    replicate
    ;

  # verbatim text, accepted by every generator at any node
  raw = text: {
    _type = "raw";
    inherit text;
  };
  isRaw = value: isAttrs value && value._type or null == "raw";

  # a kdl node with positional args, properties, and children
  kdlNode = args: props: children: {
    _type = "kdlnode";
    inherit args props children;
  };
  isKdlNode = value: isAttrs value && value._type or null == "kdlnode";

  indentOf = depth: concatStrings (replicate depth "    ");

  # renders a scalar leaf, passing raw through untouched
  valueString =
    value:
    if isRaw value then
      value.text
    else if builtins.isFloat value then
      floatString value
    else
      nixpkgs.generators.mkValueStringDefault { } value;

  # shortest float form, 0.5 instead of toString's 0.500000
  floatString = builtins.toJSON;

  # key=value lines; raw values are written as-is
  toKeyValue =
    settings:
    if isRaw settings then
      settings.text
    else
      nixpkgs.generators.toKeyValue {
        mkKeyValue = nixpkgs.generators.mkKeyValueDefault { mkValueString = valueString; } "=";
        listsAsDuplicateKeys = true;
      } settings;

  # ini with scalar globals first, then [sections]; a raw section is written verbatim without a header
  toINI =
    settings:
    let
      globals = filterAttrs (_: value: !isAttrs value) settings;
      sections = filterAttrs (_: isAttrs) settings;
      renderSection = name: body: if isRaw body then body.text else "[${name}]\n${toKeyValue body}";
    in
    if isRaw settings then
      settings.text
    else
      concatStringsSep "\n" (
        (if globals == { } then [ ] else [ (toKeyValue globals) ]) ++ mapAttrsToList renderSection sections
      );

  # pretty-printed json with two-space indent; raw values are spliced in as literal json
  toJSON =
    settings:
    let
      render =
        depth: value:
        let
          pad = indentOf' (depth + 1);
          close = indentOf' depth;
          renderAll = items: concatStringsSep ",\n" (map (item: pad + item) items);
        in
        if isRaw value then
          value.text
        else if isList value then
          if value == [ ] then "[]" else "[\n${renderAll (map (render (depth + 1)) value)}\n${close}]"
        else if isAttrs value then
          if value == { } then
            "{}"
          else
            "{\n${
              renderAll (mapAttrsToList (key: item: "${builtins.toJSON key}: ${render (depth + 1) item}") value)
            }\n${close}}"
        else
          builtins.toJSON value;
      indentOf' = depth: concatStrings (replicate depth "  ");
    in
    if isRaw settings then settings.text else render 0 settings + "\n";

  # kdl scalar: strings and numbers via json escaping, null as a bare word
  kdlValue =
    value:
    if isRaw value then
      value.text
    else if value == null then
      "null"
    else
      builtins.toJSON value;

  # node names are quoted only when kdl requires it
  kdlName =
    name:
    let
      bare = builtins.match "[A-Za-z_][A-Za-z0-9_+.:-]*" name != null;
      keyword = builtins.elem name [ "true" "false" "null" ];
    in
    if bare && !keyword then name else builtins.toJSON name;

  # renders one node; the value's shape decides args, props, children, or repetition
  kdlEntry =
    depth: name: value:
    let
      line = text: "${indentOf depth}${text}\n";
      head = args: props: concatStringsSep " " (
        [ (kdlName name) ]
        ++ map kdlValue args
        ++ mapAttrsToList (key: prop: "${key}=${kdlValue prop}") props
      );
      block = children: if children == { } then "" else " {\n${kdlBody (depth + 1) children}${indentOf depth}}";
    in
    if isRaw value then
      line value.text
    else if isKdlNode value then
      line (head value.args value.props + block value.children)
    else if isList value && value != [ ] && builtins.all (item: isAttrs item || isList item) value then
      # a list of blocks or arg lists repeats the node
      concatMapStrings (kdlEntry depth name) value
    else if isList value then
      line (head value { })
    else if isAttrs value then
      line (head [ ] { } + (if value == { } then " {}" else block value))
    else if value == null then
      line (head [ ] { })
    else
      line (head [ value ] { });

  # children of a node; raw children are emitted verbatim in place
  kdlBody = depth: nodes: concatStrings (mapAttrsToList (kdlEntry depth) nodes);

  toKDL = settings: if isRaw settings then settings.text else kdlBody 0 settings;

  # shell rc: aliases, exports, then verbatim extra text
  toShell =
    {
      aliases ? { },
      exports ? { },
      extra ? "",
    }:
    let
      quote = value: "'${builtins.replaceStrings [ "'" ] [ "'\\''" ] (toString value)}'";
      aliasLines = mapAttrsToList (name: value: "alias ${name}=${quote value}\n") aliases;
      exportLines = mapAttrsToList (name: value: "export ${name}=${if isRaw value then value.text else quote value}\n") exports;
    in
    concatStrings (aliasLines ++ exportLines) + (if isRaw extra then extra.text else extra);

  # css value: lists are comma-separated, everything else is written as-is
  cssValue = value: if isRaw value then value.text else if isList value then concatMapStringsSep ", " cssValue value else valueString value;

  # selector of a nested rule; & stands for the parent, otherwise it's a descendant
  cssSelector =
    parent: key:
    if parent == "" then
      key
    else if hasInfix "&" key then
      builtins.replaceStrings [ "&" ] [ parent ] key
    else
      "${parent} ${key}";

  # one entry as a list of rule blocks; nested rules flatten after their parent
  cssEntry =
    indent: parent: key: value:
    let
      selector = cssSelector parent key;
      isRule = item: isAttrs item && !isRaw item;
      properties = filterAttrs (_: item: !isRule item) value;
      nested = filterAttrs (_: isRule) value;
      propertyLines = concatStrings (mapAttrsToList (name: item: "${indent}  ${name}: ${cssValue item};\n") properties);
      block = if properties == { } then [ ] else [ "${indent}${selector} {\n${propertyLines}${indent}}\n" ];
    in
    if isRaw value then
      [ value.text ]
    else if hasPrefix "@" key && (isAttrs value || isList value) then
      [ "${indent}${key} {\n${concatStringsSep "\n" (cssRules "${indent}  " "" value)}${indent}}\n" ]
    else if !isAttrs value then
      # a scalar at rule level is a statement, e.g. "@define-color bg" = "#000"
      [ "${indent}${key} ${cssValue value};\n" ]
    else
      block ++ cssRules indent selector nested;

  # a list keeps cascade order; within an attrset, statements come first so @define-color precedes its uses
  cssRules =
    indent: parent: rules:
    let
      isStatement = item: !isAttrs item && !isList item;
      render = entries: concatLists (mapAttrsToList (cssEntry indent parent) entries);
    in
    if isList rules then
      concatLists (map (cssRules indent parent) rules)
    else
      render (filterAttrs (_: isStatement) rules) ++ render (filterAttrs (_: item: !isStatement item) rules);

  # css from selector -> properties, with nesting and @-blocks
  toCSS = settings: if isRaw settings then settings.text else concatStringsSep "\n" (cssRules "" "" settings);

  # toml keys are bare when toml allows it
  tomlKey = key: if builtins.match "[A-Za-z0-9_-]+" key != null then key else builtins.toJSON key;

  # inline toml value: json covers strings, numbers, and bools; attrsets become inline tables
  tomlValue =
    value:
    if isRaw value then
      value.text
    else if isList value then
      "[${concatMapStringsSep ", " tomlValue value}]"
    else if isAttrs value then
      "{ ${concatStringsSep ", " (mapAttrsToList (key: item: "${tomlKey key} = ${tomlValue item}") value)} }"
    else
      builtins.toJSON value;

  isTable = value: isAttrs value && !isRaw value;
  isTableArray = value: isList value && value != [ ] && builtins.all isTable value;

  # one table: its own pairs under a header, then its subtables and arrays of tables
  tomlTable =
    array: path: table:
    let
      header = concatMapStringsSep "." tomlKey path;
      pairs = concatStrings (
        mapAttrsToList (key: value: "${tomlKey key} = ${tomlValue value}\n") (
          filterAttrs (_: value: !isTable value && !isTableArray value) table
        )
      );
      tables = filterAttrs (_: isTable) table;
      arrays = filterAttrs (_: isTableArray) table;

      # a plain table only needs a header when it holds pairs or nothing at all
      needsHeader = path != [ ] && (array || pairs != "" || tables == { } && arrays == { });
      own = if needsHeader then (if array then "[[${header}]]\n" else "[${header}]\n") + pairs else pairs;
      nested =
        mapAttrsToList (key: tomlTable false (path ++ [ key ])) tables
        ++ concatLists (mapAttrsToList (key: map (tomlTable true (path ++ [ key ]))) arrays);
    in
    concatStringsSep "\n" (builtins.filter (text: text != "") ([ own ] ++ nested));

  # toml with top-level pairs first, then [tables] and [[arrays of tables]]
  toTOML = settings: if isRaw settings then settings.text else tomlTable false [ ] settings;

  generators = {
    css = toCSS;
    ini = toINI;
    kdl = toKDL;
    json = toJSON;
    keyValue = toKeyValue;
    shell = toShell;
    toml = toTOML;
    raw = settings: if isRaw settings then settings.text else settings;
  };

  # renders one file's settings; strings and raw are always verbatim
  renderFile =
    format: settings:
    if builtins.isString settings then
      settings
    else if isRaw settings then
      settings.text
    else
      generators.${format} settings;

  # a program's config as a list of { name, key, content, executable, scope }
  program =
    name:
    {
      format ? "raw",
      settings ? null,
      files ? { main = settings; },
      executable ? false,
      path ? null,
      scope ? "user",
    }:
    mapAttrsToList (key: fileSettings: {
      inherit name executable scope;
      key = if path != null && key == "main" then path else key;
      # format may be one string or an attrset keyed like files
      content = renderFile (if isAttrs format then format.${key} else format) fileSettings;
    }) files;
in
nixpkgs
// {
  inherit
    raw
    isRaw
    program
    toCSS
    toINI
    toJSON
    toKDL
    toKeyValue
    toShell
    toTOML
    ;
  kdl.node = kdlNode;
  mawVersion = "0.1.0";
}

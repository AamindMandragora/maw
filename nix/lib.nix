let
  nixpkgs = import ./nixpkgs-lib;
  inherit (nixpkgs)
    concatLists
    concatMapStrings
    concatMapStringsSep
    concatStrings
    concatStringsSep
    filterAttrs
    genAttrs
    hasInfix
    hasPrefix
    isAttrs
    isList
    mapAttrs
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

  # the control characters a nix string can hold (all but nul), with their codes
  controlCodes = builtins.genList (index: index + 1) 31 ++ [ 127 ];
  controlChar = code: builtins.fromJSON ''"\u${lowerHex 4 code}"'';

  # a number as lowercase hex, zero-padded to width digits
  lowerHex =
    width: number:
    let
      digits = nixpkgs.toLower (nixpkgs.toHexString number);
    in
    concatStrings (replicate (width - builtins.stringLength digits) "0") + digits;

  # escapes a string: named escapes first, then every other control character through unicode
  escapeWith =
    named: unicode: text:
    let
      others = builtins.filter (code: !(named ? ${controlChar code})) controlCodes;
    in
    builtins.replaceStrings (builtins.attrNames named ++ map controlChar others) (
      builtins.attrValues named ++ map unicode others
    ) text;

  # a nix path renders as its literal text, never copied into the store
  unpath = value: if builtins.isPath value then toString value else value;

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
          builtins.toJSON (unpath value);
      indentOf' = depth: concatStrings (replicate depth "  ");
    in
    if isRaw settings then settings.text else render 0 settings + "\n";

  # a kdl string literal: backslash, quote, and whitespace escapes, other control characters as \u{hex}
  kdlString =
    text:
    "\"${
      escapeWith {
        "\\" = "\\\\";
        "\"" = "\\\"";
        "\n" = "\\n";
        "\r" = "\\r";
        "\t" = "\\t";
      } (code: "\\u{${nixpkgs.toLower (nixpkgs.toHexString code)}}") text
    }\"";

  # kdl scalar: strings escaped for kdl, numbers and bools as json, null as a bare word
  kdlValue =
    value:
    if isRaw value then
      value.text
    else if value == null then
      "null"
    else if builtins.isString value || builtins.isPath value then
      kdlString (toString value)
    else
      builtins.toJSON value;

  # node and property names are quoted only when kdl requires it
  kdlName =
    name:
    let
      bare = builtins.match "[A-Za-z_][A-Za-z0-9_+.:-]*" name != null;
      keyword = builtins.elem name [ "true" "false" "null" ];
    in
    if bare && !keyword then name else kdlString name;

  # renders one node; the value's shape decides args, props, children, or repetition
  kdlEntry =
    depth: name: value:
    let
      line = text: "${indentOf depth}${text}\n";
      head = args: props: concatStringsSep " " (
        [ (kdlName name) ]
        ++ map kdlValue args
        ++ mapAttrsToList (key: prop: "${kdlName key}=${kdlValue prop}") props
      );
      block = children: if children == { } then "" else " {\n${kdlBody (depth + 1) children}${indentOf depth}}";
      isBlock = item: isAttrs item && !isRaw item || isList item;
    in
    if isRaw value then
      line value.text
    else if isKdlNode value then
      line (head value.args value.props + block value.children)
    else if isList value && value != [ ] && builtins.all (item: isAttrs item || isList item) value then
      # a list of blocks or arg lists repeats the node
      concatMapStrings (kdlEntry depth name) value
    else if isList value && builtins.any isBlock value then
      throw "maw: toKDL node ${name} mixes values and blocks in one list; use lib.kdl.node"
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

  # a selector list split at its top-level commas, so :is(.a, .b) stays whole
  splitSelectors =
    text:
    let
      depthChange = char: if char == "(" || char == "[" then 1 else if char == ")" || char == "]" then -1 else 0;
      step =
        state: char:
        if char == "," && state.depth == 0 then
          state // { parts = state.parts ++ [ state.current ]; current = ""; }
        else
          state // { current = state.current + char; depth = state.depth + depthChange char; };
      final = builtins.foldl' step { parts = [ ]; current = ""; depth = 0; } (nixpkgs.stringToCharacters text);
    in
    map nixpkgs.trim (final.parts ++ [ final.current ]);

  # selector of a nested rule; & stands for each parent, otherwise it's a descendant of each
  cssSelector =
    parent: key:
    let
      nest = child: map (outer: if hasInfix "&" child then builtins.replaceStrings [ "&" ] [ outer ] child else "${outer} ${child}") (splitSelectors parent);
    in
    if parent == "" then key else concatStringsSep ", " (nixpkgs.concatMap nest (splitSelectors key));

  # the inside of an @-block: under a rule its properties stay wrapped in that rule's selector
  cssAtBody =
    indent: parent: value:
    let
      isProperty = key: item: !hasPrefix "@" key && !(isAttrs item && !isRaw item);
      propertyLines = concatStrings (mapAttrsToList (name: item: "${indent}${name}: ${cssValue item};\n") (filterAttrs isProperty value));
      rest = filterAttrs (key: item: !isProperty key item) value;
    in
    if isList value then
      concatLists (map (cssAtBody indent parent) value)
    else if parent != "" then
      cssEntry indent "" parent value
    else
      (if propertyLines == "" then [ ] else [ propertyLines ]) ++ cssRules indent "" rest;

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
      [ "${indent}${key} {\n${concatStringsSep "\n" (cssAtBody "${indent}  " parent value)}${indent}}\n" ]
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

  # a toml basic string: json escaping plus del, which toml forbids raw
  tomlString = text: builtins.replaceStrings [ (controlChar 127) ] [ "\\u007F" ] (builtins.toJSON text);

  # toml keys are bare when toml allows it
  tomlKey = key: if builtins.match "[A-Za-z0-9_-]+" key != null then key else tomlString key;

  # inline toml value under key: json covers numbers and bools; attrsets become inline tables
  tomlValue =
    key: value:
    if isRaw value then
      value.text
    else if value == null then
      throw "maw: toTOML has no null (key ${key}); leave the key out"
    else if builtins.isString value || builtins.isPath value then
      tomlString (toString value)
    else if isList value then
      "[${concatMapStringsSep ", " (tomlValue key) value}]"
    else if isAttrs value then
      "{ ${concatStringsSep ", " (mapAttrsToList (name: item: "${tomlKey name} = ${tomlValue name item}") value)} }"
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
        mapAttrsToList (key: value: "${tomlKey key} = ${tomlValue key value}\n") (
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

  # a program's config as a list of { name, key, content, executable, scope, reload }
  program =
    name:
    {
      format ? "raw",
      settings ? null,
      files ? { main = settings; },
      executable ? false,
      path ? null,
      scope ? "user",
      # a command that makes the running program read its changed config; null uses the registry's
      reload ? null,
    }:
    mapAttrsToList (key: fileSettings: {
      inherit name executable scope reload;
      key = if path != null && key == "main" then path else key;
      # format may be one string or an attrset keyed like files
      content = renderFile (if isAttrs format then format.${key} else format) fileSettings;
    }) files;

  # a flatpak app's permissions and environment, written as flatpak's own user overrides for that app id; lists are
  # written the way flatpak does, each item followed by ;, and an item starting with ! takes a permission away
  flatpak =
    id:
    {
      env ? { },
      sockets ? [ ],
      filesystems ? [ ],
      devices ? [ ],
      shared ? [ ],
      talk ? [ ],
      own ? [ ],
    }:
    let
      joined = concatMapStrings (item: "${item};");
      context = filterAttrs (_: list: list != [ ]) { inherit sockets filesystems devices shared; };
      bus = genAttrs talk (_: "talk") // genAttrs own (_: "own");
      sections = filterAttrs (_: section: section != { }) {
        Context = mapAttrs (_: joined) context;
        Environment = env;
        "Session Bus Policy" = bus;
      };
    in
    program "flatpak" {
      format = "ini";
      path = id;
      settings = sections;
    };

  # a gvariant string as g_variant_print writes it: double quotes when it holds a single quote, else single
  gvariantString =
    text:
    let
      quote = if hasInfix "'" text then "\"" else "'";
      named = {
        "\\" = "\\\\";
        ${quote} = "\\${quote}";
        ${controlChar 7} = "\\a";
        ${controlChar 8} = "\\b";
        ${controlChar 12} = "\\f";
        "\n" = "\\n";
        "\r" = "\\r";
        "\t" = "\\t";
        ${controlChar 11} = "\\v";
      };
    in
    quote + escapeWith named (code: "\\u${lowerHex 4 code}") text + quote;

  # a nix value as gvariant text, exactly as dconf read prints it; an empty list is typed @as, so
  # other empty array types need lib.raw
  toGVariant = value: if value == [ ] then "@as []" else gvariantValue value;
  gvariantValue =
    value:
    if isRaw value then
      value.text
    else if builtins.isBool value then
      (if value then "true" else "false")
    else if builtins.isInt value then
      toString value
    else if builtins.isFloat value then
      builtins.toJSON value
    else if builtins.isString value then
      gvariantString value
    else if builtins.isList value then
      "[${concatMapStringsSep ", " gvariantValue value}]"
    else
      throw "maw: dconf values are strings, numbers, booleans, lists, or lib.raw GVariant text";

  # colors as "#rrggbb" (or without the #), for deriving one from another: a hue turned, lightness moved, an ansi escape
  color =
    let
      # the color itself, or an error when it isn't six hex digits
      checked =
        hex:
        if builtins.isString hex && builtins.match "#?[0-9A-Fa-f]{6}" hex != null then
          hex
        else
          throw "maw: lib.color wants #rrggbb, got ${if builtins.isString hex then hex else builtins.typeOf hex}";

      # 0-255 channels, and back to two hex digits
      channels =
        hex:
        let
          digits = nixpkgs.removePrefix "#" (checked hex);
          byte = at: nixpkgs.fromHexString (builtins.substring at 2 digits);
        in
        {
          r = byte 0;
          g = byte 2;
          b = byte 4;
        };
      twoDigits =
        value:
        let
          text = nixpkgs.toLower (nixpkgs.toHexString value);
        in
        if builtins.stringLength text < 2 then "0${text}" else text;
      clamp = low: high: value: if value < low then low else if value > high then high else value;
      round = value: builtins.floor (value + 0.5);

      # hue in degrees, saturation and lightness 0-1
      toHsl =
        hex:
        let
          c = channels hex;
          r = c.r / 255.0;
          g = c.g / 255.0;
          b = c.b / 255.0;
          high = nixpkgs.max r (nixpkgs.max g b);
          low = nixpkgs.min r (nixpkgs.min g b);
          delta = high - low;
          l = (high + low) / 2;
          s = if delta == 0 then 0.0 else if l > 0.5 then delta / (2 - high - low) else delta / (high + low);
          sector =
            if delta == 0 then 0.0
            else if high == r then (g - b) / delta + (if g < b then 6 else 0)
            else if high == g then (b - r) / delta + 2
            else (r - g) / delta + 4;
        in
        {
          h = sector * 60;
          inherit s l;
        };
      fromHsl =
        { h, s, l }:
        let
          hue = (h - 360 * builtins.floor (h / 360.0)) / 360.0;
          q = if l < 0.5 then l * (1 + s) else l + s - l * s;
          p = 2 * l - q;
          channel =
            t:
            let
              t' = if t < 0 then t + 1 else if t > 1 then t - 1 else t;
            in
            if t' < 1 / 6.0 then p + (q - p) * 6 * t'
            else if t' < 0.5 then q
            else if t' < 2 / 3.0 then p + (q - p) * (2 / 3.0 - t') * 6
            else p;
          byte = t: twoDigits (clamp 0 255 (round (channel t * 255)));
        in
        byte (hue + 1 / 3.0) + byte hue + byte (hue - 1 / 3.0);

      # the result keeps the input's # or lack of one
      like = hex: result: if nixpkgs.hasPrefix "#" (checked hex) then "#${result}" else result;
    in
    {
      inherit toHsl;
      rgb = channels;
      # another hue at the same saturation and lightness: rotate 35 "#ffb4ab" turns a soft red amber
      rotate = degrees: hex: let hsl = toHsl hex; in like hex (fromHsl (hsl // { h = hsl.h + degrees; }));
      # lighter (positive) or darker (negative), by lightness points 0-1
      lighten = amount: hex: let hsl = toHsl hex; in like hex (fromHsl (hsl // { l = clamp 0.0 1.0 (hsl.l + amount); }));
      # the sgr parameters of a 24-bit foreground color, for \e[...m
      ansi = hex: let c = channels hex; in "38;2;${toString c.r};${toString c.g};${toString c.b}";
    };

  # desktop settings in dconf, by path then key: { "org/gnome/desktop/interface".color-scheme = "prefer-dark"; }
  dconf = settings: [
    {
      name = "dconf";
      key = "dconf";
      scope = "user";
      content = "";
      executable = false;
      dconf = nixpkgs.concatMapAttrs (
        path: keys: nixpkgs.mapAttrs' (key: value: nixpkgs.nameValuePair "/${path}/${key}" (toGVariant value)) keys
      ) settings;
    }
  ];

  # a supervised service, described abstractly; maw's init backend writes its run and log files
  service =
    name:
    {
      scope ? "system",
      run,
      log ? true,
      enable ? true,
      env ? { },
      core ? false,
    }:
    if !builtins.elem scope [ "system" "user" ] then
      throw "maw: service ${name} has scope ${toString scope}; use \"system\" or \"user\""
    else if core && scope != "user" then
      throw "maw: service ${name} is core; only user services can be"
    else
      [
        {
          inherit name scope;
          key = "service";
          content = "";
          executable = false;
          service = {
            inherit run log enable core;
            env = builtins.mapAttrs (_: toString) env;
          };
        }
      ];
in
nixpkgs
// {
  inherit
    raw
    isRaw
    program
    service
    flatpak
    dconf
    color
    toGVariant
    toCSS
    toINI
    toJSON
    toKDL
    toKeyValue
    toShell
    toTOML
    ;
  kdl.node = kdlNode;
  mawVersion = "0.2.3";
}

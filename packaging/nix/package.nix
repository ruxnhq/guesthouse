# Guesthouse's release, unpacked and patched to run on NixOS. A build from source has no Windows display
# driver, as that needs Windows and Microsoft's kit, so this is the release itself and not a rebuild.
{
  lib,
  stdenvNoCC,
  fetchurl,
  autoPatchelfHook,
  patchelfUnstable,
  addDriverRunpath,
  wrapGAppsHook4,
  glib,
  gtk4,
  libadwaita,
  librsvg,
  pipewire,
  libepoxy,
  libdrm,
  libgbm,
  libglvnd,
  libcap_ng,
  stdenv,
  # What the app runs. It takes any of the disc makers, and isoinfo comes with cdrkit's.
  qemu-utils,
  _7zz,
  passt,
  runCommand,
  cdrkit,
  swtpm,
  openssl,
  openssh,
  bubblewrap,
  gcc,
  coreutils,
  gnutar,
  procps,
  curl,
}:

let
  # The app runs it as 7z, and 7zz is all the package has.
  sevenZip = runCommand "7z" { } ''
    mkdir -p $out/bin
    ln -s ${_7zz}/bin/7zz $out/bin/7z
  '';
  # What the app runs by name.
  toolPath = lib.makeBinPath [
    qemu-utils
    sevenZip
    passt
    cdrkit
    swtpm
    openssl
    openssh
    bubblewrap
    gcc
    coreutils
    gnutar
    procps
    curl
  ];
  version = "0.7.7";
  # One folder for everything the app finds by where its own program is, as it does with the guest files.
  home = "$out/lib/guesthouse";
  # The macOS part, which other installs fetch the first time a Mac is made. Its programs are built for
  # a system with /lib64's loader, which NixOS hasn't, and even with nix-ld they can't run in the jail.
  # So it comes with the package, patched like the rest, in the program folder where the app looks
  # first. Its number and hash are app-core's macos_part.rs.
  macosPart = fetchurl {
    url = "https://github.com/ruxnhq/guesthouse/releases/download/macos-part-1/guesthouse-macos-part-1-x86_64.tar.xz";
    hash = "sha256-V4cQ5mL8dVsxBcw4AN411uroEs3XFmJKwCutMimBqLw=";
  };
in
stdenvNoCC.mkDerivation {
  pname = "guesthouse";
  inherit version;

  src = fetchurl {
    url = "https://github.com/ruxnhq/guesthouse/releases/download/v${version}/guesthouse-${version}-x86_64.tar.xz";
    hash = "sha256-joFe8A+9/I1mrDARFUIRAD1jQ8auaaTxh1hpILC6398=";
  };
  sourceRoot = "guesthouse-${version}-x86_64";

  nativeBuildInputs = [
    # The release's programs break when patchelf 0.15, which is the default, rewrites them.
    patchelfUnstable
    autoPatchelfHook
    addDriverRunpath
    wrapGAppsHook4
  ];

  # What the programs are linked against. The release bundles QEMU's other libraries.
  buildInputs = [
    glib
    gtk4
    libadwaita
    librsvg
    pipewire
    libepoxy
    libdrm
    libgbm
    libcap_ng
    stdenv.cc.cc.lib
  ];

  # QEMU loads EGL when a machine starts, rather than when it is linked.
  runtimeDependencies = [ libglvnd ];

  # The macOS part's QEMU has SDL linked in, and SDL's notes name libraries it may load. autoPatchelf
  # looks for those too, but a Mac's screen goes through D-Bus and never loads them.
  autoPatchelfIgnoreMissingDeps = [
    "libGLES_CM.so.1"
    "libXtst.so.6"
    "libXss.so.1"
    "libdecor-0.so.0"
    "libsteam_api.so"
  ];

  dontConfigure = true;
  dontBuild = true;
  dontStrip = true;
  dontPatchELF = true;
  # The scripts under linux/ run in a guest, where a store path in the first line would lead nowhere.
  dontPatchShebangs = true;
  dontAutoPatchelf = true;
  dontWrapGApps = true;

  installPhase = ''
    runHook preInstall

    mkdir -p ${home} $out/bin $out/share/applications
    # The install script is how the app tells an install of its own from any other, and it updates the
    # first kind. This one is the package manager's to update.
    rm install.sh uninstall.sh
    # These are for install.sh: the second app is for a GTK older than 4.16, and the list is for
    # checking the host's libraries.
    rm guesthouse-no-hdr needs-symbols

    mv share/io.github.xnixy.Guesthouse.desktop $out/share/applications/
    mv share/icons $out/share/icons
    rmdir share
    substituteInPlace $out/share/applications/io.github.xnixy.Guesthouse.desktop \
      --replace-fail 'Exec=guesthouse' "Exec=$out/bin/guesthouse"

    cp -r . ${home}
    mkdir ${home}/macos
    tar -xJf ${macosPart} -C ${home}/macos --strip-components=1

    runHook postInstall
  '';

  # Only what runs on this machine. The guest's files (guest/, linux/, display-driver/) are copied into
  # a machine, and QEMU's firmware for other architectures isn't run at all, so none of those are touched.
  postFixup = ''
    host_files=$(find ${home} -maxdepth 1 -type f -perm -u+x \( -name 'guesthouse*' -o -name intel-probe \))
    qemu=${home}/qemu/build
    macos=${home}/macos

    # Apart, as autoPatchelf also takes libraries from the files it patches, and the app mustn't get
    # QEMU's glib in place of the one GTK is built against.
    autoPatchelf $host_files
    autoPatchelf $qemu
    autoPatchelf $macos/qemu $macos/reims-util
    # The bridge helper runs setuid root, so it keeps to libraries in the store.
    addDriverRunpath $host_files $(find $qemu -type f ! -name qemu-bridge-helper) $macos/qemu/bin/qemu-system-x86_64

    makeWrapper ${home}/guesthouse $out/bin/guesthouse \
      "''${gappsWrapperArgs[@]}" \
      --prefix PATH : ${toolPath}
  '';

  passthru = { inherit toolPath; };

  meta = {
    description = "Windows and Linux machines on KVM, with the real GPU driver";
    homepage = "https://github.com/ruxnhq/guesthouse";
    license = lib.licenses.gpl2Plus;
    platforms = [ "x86_64-linux" ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = "guesthouse";
  };
}

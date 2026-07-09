{ pkgs, ... }:
{
  # llmlang (lllc) — environnement de dev reproductible (GUI-PRO-012).
  # Un seul fichier épingle Rust, l'oracle Z3 et Postgres — remplace le binaire
  # Z3 vendorisé (vendor/z3, gitignoré) par un Z3 nixpkgs déterministe, et
  # fournit Postgres SANS docker pour le vertical APS3D (DEC-LLL-066, étape 2).

  languages.rust = {
    enable = true;
    channel = "stable";
  };

  packages = with pkgs; [
    z3            # oracle SMT — source de LLL_Z3 (remplace vendor/z3/bin/z3)
    pkg-config
    gcc           # rusqlite(bundled) compile SQLite en C dans les exemples Db
    openssl
    openssl.dev
  ];

  env = {
    # L'oracle Z3 vient de nix (épinglé par devenv.lock) — plus de téléchargement
    # ni de binaire vendorisé. Prioritaire sur PATH et sur vendor/z3.
    LLL_Z3 = "${pkgs.z3}/bin/z3";
  };

  # Postgres reproductible pour le vertical APS3D (étape 2 : swap SQLite→Postgres) —
  # débloque le gate infra sans docker. `devenv up` démarre le service ; les tests
  # cargo s'y connectent via DATABASE_URL.
  services.postgres = {
    enable = true;
    initialDatabases = [ { name = "aps3d_rules"; } ];
    listen_addresses = "127.0.0.1";
    port = 5432;
  };

  enterShell = ''
    echo "llmlang devenv — Rust $(rustc --version | cut -d' ' -f2) · Z3 $(z3 --version | cut -d' ' -f3)"
    echo "  LLL_Z3=$LLL_Z3"
  '';
}

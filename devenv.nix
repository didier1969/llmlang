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
  # débloque le gate infra sans docker. `devenv up` démarre le service ; l'exemple
  # `aps3d_rules_persist_pg.lll` s'y connecte via une conn-string EXPLICITE et
  # SANS credentials machine-spécifiques :
  #     host=127.0.0.1 port=5442 user=aps3d dbname=aps3d_rules
  # - port 5442 (pas le 5432 par défaut, souvent déjà occupé — évite l'auto-bump
  #   non-déterministe de devenv, garde la conn-string committée reproductible) ;
  # - rôle applicatif `aps3d` (pas le superuser = nom d'utilisateur OS, non portable)
  #   créé au premier `initdb` par `initialScript` ; auth `trust` en local → pas de
  #   mot de passe. Le port/rôle sont du CONFIG backend-spécifique : le CONTRAT
  #   (ops/types de l'effet `Db`) est identique à std/db.lll — seule la config diffère.
  services.postgres = {
    enable = true;
    # `aps3d_rules` : le vertical single-backend (aps3d_rules_persist_pg.lll).
    # `aps3d_rules_multi` : la base Postgres du démo « deux backends vivants » (Voie C,
    #   aps3d_rules_multi.lll / REQ-LLL-094) — isolée pour ne pas croiser le single-backend.
    initialDatabases = [ { name = "aps3d_rules"; } { name = "aps3d_rules_multi"; } ];
    initialScript = "CREATE ROLE aps3d WITH LOGIN SUPERUSER;";
    listen_addresses = "127.0.0.1";
    port = 5442;
  };

  enterShell = ''
    echo "llmlang devenv — Rust $(rustc --version | cut -d' ' -f2) · Z3 $(z3 --version | cut -d' ' -f3)"
    echo "  LLL_Z3=$LLL_Z3"
  '';
}

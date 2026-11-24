# VeloEdit (Türkçe özet)

Sublime Text mantığında, Rust ile yazılmış hızlı, hafif, GPU hızlandırmalı Linux metin editörü.
Ayrıntılı İngilizce açıklama: [README.md](README.md). Mimari notlar (Türkçe): [ARCHITECTURE.md](ARCHITECTURE.md).
**[100 saniyelik tanıtım videosu](https://tkinali.github.io/veloedit/#tour)** · **[Sublime Text, VS Code, Kate, Zed, Helix ile karşılaştırma](https://tkinali.github.io/veloedit/#compare)**

## Kurulum

**Hazır paket** (GitHub Releases): `tar xzf veloedit-*-linux.tar.gz && cd veloedit-*-linux && ./install.sh`
(`~/.local` altına kurar, root gerekmez; uygulama menüsüne ve "Birlikte aç"a eklenir; kaldırmak için `./install.sh --uninstall`).

**Kaynaktan**: Rust 1.92+ ve bir C derleyicisi gerekir (`sudo dnf install gcc` / `sudo apt install build-essential`).

```bash
cargo build --release && packaging/install.sh
```

## Kullanım

```bash
veloedit                    # son oturumu geri yükler
veloedit dosya.php klasor/  # çalışan pencerede açılır (tek pencere); --new-window ile ayrı pencere
```

Kısayollar: `Ctrl+O` dosya aç · `Ctrl+Shift+O` klasör ekle · `Ctrl+P` dosyaya git · `Ctrl+Shift+P` komut paleti ·
`Ctrl+Shift+F` projede ara · `Ctrl+Shift+[` / `]` katla / aç · `Ctrl+B` kenar çubuğu · `Ctrl+,` ayarlar.

Ağ bağlantılı dosya sistemlerinde (SFTP/sshfs, VPN) dosya düzenlemek güvenlidir: koparsa pencere donmaz, diskten yeniden yükleme `Ctrl+Z` ile geri alınır ve boş okunan dosya dolu bir buffer'ı silmez; donmuş bir pencere sonraki başlatmayı engellemez.

Sözdizimi ve biçim (View menüsü): **Syntax…** (ya da durum çubuğundaki dil adına tıklamak) tüm dillerin listesini açar, seçileni uygular; *Auto Detect* kararı dosya adına/içeriğe geri verir. Yeni bir sekmeye yapıştırılan ve bir web sayfası gibi başlayan metin (`<!DOCTYPE html>`, `<html>`, `<?xml`, `#!/bin/bash` …) hemen renklenir. **Convert Indentation to Spaces / Tabs** girintiyi dönüştürür (ayarlardaki sekme boyutuyla), **Line Endings: Unix (LF) / Windows (CRLF)** satır sonlarını dönüştürür; ikisi de tek `Ctrl+Z` adımıdır.

Ayarlar (`Ctrl+,`): tema, yazı/sekme boyutu, satır kaydırma, minimap ve **Trim on save** (kaydederken satır sonlarındaki boşluk ve tab'ları siler; Markdown'daki iki boşluklu satır sonları korunur; varsayılan kapalı, `Ctrl+Z` ile geri alınır).

Lisans: MIT ([LICENSE](LICENSE)).

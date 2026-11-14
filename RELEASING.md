# Sürüm çıkarma rehberi

Yeni bir sürüm çıkarmak, bir **etiket (tag) göndermek** demektir. Gerisini GitHub Actions yapar:
Linux için iki işlemci mimarisinde (x86_64, aarch64) tarball, `.deb`, `.rpm` ve `.AppImage`, Windows için (x86_64) zip derler;
paketleri ve sağlama dosyalarını (`.sha256`) bir **GitHub Release** olarak yayınlar, açıklama
metnini `CHANGELOG.md`'den alır. Web sitesi
(<https://tkinali.github.io/veloedit/>) en son sürümü ve indirme bağlantılarını GitHub'dan kendisi
okur; siteye dokunmana gerek yok.

## Her sürümde yapılacaklar

1. **Sürüm numarasını yükselt**: `Cargo.toml` içindeki `version = "0.2.0"`. Sonra `cargo build --release`
   çalıştır; `Cargo.lock` kendiliğinden güncellenir.
   Numara: hata düzeltmesi `0.1.0 → 0.1.1`, yeni özellik `0.1.0 → 0.2.0`.
2. **`CHANGELOG.md`'ye yeni bölüm ekle** (en üste, eskilerin üstüne):

   ```markdown
   ## 0.2.0 - 2026-12-01

   ### Added
   - Yeni özellik ...

   ### Fixed
   - Düzeltilen hata ...
   ```

   Başlık tam olarak `## <sürüm> - <tarih>` biçiminde olmalı. Bu bölümün metni, GitHub'daki sürüm
   açıklaması olur. Bölüm yoksa iş akışı hata verir ve **hiçbir şey yayınlanmaz**.
3. **Yerelde dene**:

   ```bash
   export PATH=$HOME/.cargo/bin:$PATH
   cargo test --release          # 185+ test
   scripts/release-notes.sh 0.2.0   # açıklama metni doğru çıkıyor mu?
   ```
4. **Commit'le ve gönder**, sonra GitHub'da **Actions** sekmesinde `CI` çalışmasının yeşil olmasını bekle:

   ```bash
   git add -A
   git commit -m "Release 0.2.0"
   git push
   ```
5. **Etiketle ve gönder** (bu, yayını başlatır):

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
6. **Actions → Release** çalışmasını izle (yaklaşık 10-15 dakika; ilk seferde Rust önbelleği boş olduğu
   için daha uzun sürebilir). Bitince **Releases** sayfasında `v0.2.0` görünür:
   `veloedit-0.2.0-x86_64-linux.tar.gz`, `veloedit-0.2.0-aarch64-linux.tar.gz`,
   `veloedit_0.2.0-1_amd64.deb`, `veloedit_0.2.0-1_arm64.deb`, `veloedit-0.2.0-1.x86_64.rpm`,
   `veloedit-0.2.0-1.aarch64.rpm`, `VeloEdit-0.2.0-x86_64.AppImage`, `VeloEdit-0.2.0-aarch64.AppImage`,
   `veloedit-0.2.0-x86_64-windows.zip` ve her birinin `.sha256` dosyası (toplam 9 paket, 18 dosya).
7. **Kontrol et**: Linux paketini indirip kur (`tar xzf ... && ./install.sh`), `veloedit --version` doğru
   numarayı yazsın. Windows zip'ini bir Windows bilgisayarda aç, `veloedit.exe` açılsın ve **Help → About**'ta
   sürüm doğru görünsün (Windows'ta `--version` konsola yazmaz, çünkü program konsolsuz bir GUI programıdır). Web sitesindeki "Son sürüm" birkaç dakika içinde güncellenir.

Etiket **her zaman** `v` + `Cargo.toml`'daki sürüm olmalıdır (`0.2.0` → `v0.2.0`); tutmazsa iş akışı
daha derlemeye başlamadan durur.

## Önce denemek istersen (kuru çalıştırma)

GitHub'da **Actions → Release → Run workflow**: derlemeyi yapar, tarball'ları o çalışmanın
**Artifacts** bölümüne koyar, ama **hiçbir şey yayınlamaz**. Etiket atmadan önce derlemenin
çalıştığından emin olmak için iyi bir yol.

## Ön sürüm (beta / rc)

`v0.3.0-rc1` gibi tireli bir etiket otomatik olarak **pre-release** işaretlenir ve sitede "son sürüm"
olarak gösterilmez. (`Cargo.toml`'da `version = "0.3.0-rc1"` ve CHANGELOG'da `## 0.3.0-rc1 - ...` olmalı.)

## Bir şeyler ters giderse

- **Release iş akışı kırmızı**: Actions'ta çalışmaya tıkla, hatalı adımın günlüğüne bak. Sorunu düzeltip
  commit'ledikten sonra etiketi yeniden koy:

  ```bash
  git push --delete origin v0.2.0     # uzaktaki etiketi sil
  git tag -d v0.2.0                   # yerel etiketi sil
  # ... düzeltmeyi commit'le ve push et ...
  git tag v0.2.0 && git push origin v0.2.0
  ```
- **Yanlış bir sürüm yayınlandı**: GitHub'da Releases → ilgili sürüm → *Delete*, sonra etiketi yukarıdaki gibi
  sil. Gerçekten kullanıcıların elinde olan bir sürümü silmek yerine düzeltip bir sonraki numarayla
  (`0.2.1`) yeniden yayınlamak daha dürüsttür.
- **Elle yükleme (acil durum)**: yerelde `scripts/package.sh` bir tarball üretir (`dist/`), ama bu makinede
  derlenen ikili **bu makinenin glibc'sini** (2.43) gerektirir; eski dağıtımlarda çalışmaz. Dağıtılacak
  paketler her zaman Release iş akışından (Ubuntu 22.04) gelmelidir.

## Web sitesi

`docs/` klasörü sitedir (`docs/index.html` + `docs/screenshots/`). `main` dalına push edince GitHub Pages
otomatik yeniden yayınlar (1-2 dakika). Yeni ekran görüntüsü eklemek için PNG'yi `docs/screenshots/`
içine koy ve `index.html`'de bir `<img>` ekle. Metinler iki dillidir: her metin `<span class="t" lang="en">`
ve `<span class="t" lang="tr">` çiftidir; ikisini birlikte güncelle.

İlk kurulumda (bir kez): GitHub'da **Settings → Pages → Build and deployment → Source: Deploy from a branch →
Branch: `main`, klasör `/docs` → Save**.

## Yararlı kontroller

```bash
git tag --list                      # mevcut etiketler
git log --oneline v0.1.0..HEAD      # son sürümden beri neler değişti (CHANGELOG'u yazarken işe yarar)
gh release list                     # (gh kuruluysa) yayınlanmış sürümler
```

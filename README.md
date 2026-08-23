# FoxShot

Ekran yakalama, işaretleme ve paylaşma aracı. macOS, Linux ve Windows için, Rust ile yazılıyor.

> **Durum: pre-alpha.** Henüz çalışan bir binary yok. Bu depoda şu anda mimari, tasarım ve
> Core iskeleti var. Yol haritası `docs/diagrams/04-insa-adimlari.drawio` dosyasında.

## Ne yapacak

- Bölge, pencere, tam ekran ve kaydırmalı çekim
- Ekran kaydı ve GIF
- Çekimin üstünde 17 işaret tipiyle düzenleme — alttaki görüntüye asla dokunmadan
- Cloudflare R2, Amazon S3 ve ücretsiz servislere yükleme, linki panoya
- OCR, QR, renk seçici gibi araçlar

## Mimari

Tek Core, üç platform adaptörü. Core tüm davranışı sahiplenir ve içinde tek bir
`cfg(target_os)` dalı bulunmaz; platforma özgü her şey `core::platform` trait'lerinin
arkasındadır. Core'da yapılan bir değişiklik üç platforma birden ulaşır.

Core, her adaptör ve her özellik modülü kendi sürümünü taşır ve ayrı güncellenir.
Açılışta `updates.json` okunur ve yeni sürüm varsa bildirilir.

Diyagramlar: `docs/diagrams/`
- `01a-crate-grafi` — crate bağımlılık grafiği
- `01b-core-siniflari` — Core'un iç sınıf haritası
- `02-yakalama-akisi` — kısayoldan linke kadar akış
- `03-modul-guncelleme` — modül kayıt defteri ve güncelleme kontrolü
- `04-insa-adimlari` — inşa sırası (S0→S10)

## Belgeler

- `PRODUCT.md` — ürün gerçeği, kullanıcılar, kapsam
- `DESIGN.md` — görsel sistem
- `design/foxshot.html` — tıklanabilir tasarım prototipi (157 senaryo)
- `FACTORY.md` — nasıl çalışıyoruz

## Lisans

PolyForm Noncommercial 1.0.0 — `LICENSE.md`. Okuyabilir, değiştirebilir ve
**paylaşabilirsin**; ticari kullanım hakkı Metanetsoft'a aittir. Sade dille
açıklaması: `LICENSING.md`.

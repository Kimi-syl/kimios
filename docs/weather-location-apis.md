# Weather APIs + Android Location — Research Report (KimiOS Weather app)

Date: 2026-11-19. RESEARCH ONLY — no code changes. All endpoints below were
live-verified from this environment on 2026-11-19 unless marked otherwise.
Context: KimiOS libOS Weather app fetches Open-Meteo via a Java
`HttpURLConnection` HTTPS syscall, parses JSON in no_std Rust; location via
`LocationManager.getLastKnownLocation` fails on the Andarium virtual device
(Android 16 / SDK 37, app targetSdk 28).

---

## 1. HKO (Hong Kong Observatory) Open Data API — free, no key, HK-only

### 1.1 Endpoint and parameters (verified live)

Base (single endpoint for all weather dataTypes):

```
GET https://data.weather.gov.hk/weatherAPI/opendata/weather.php
```

| Param      | Accepted values                                            | Notes                          |
|------------|------------------------------------------------------------|--------------------------------|
| `dataType` | `rhrread`, `flw`, `fnd`, `warnsum`, `warningInfo`, `swt`   | required                       |
| `lang`     | `en` (default), `tc` (Traditional Chinese), `sc` (Simplified) | optional                    |

- `rhrread` = Current Weather Report (hourly + on update)
- `flw` = Local Weather Forecast (today/tonight text; hourly + on update)
- `fnd` = 9-day Weather Forecast (twice daily + on update)
- `warnsum` = Weather Warning Summary (on update)
- `warningInfo` = detailed warning info; `swt` = Special Weather Tips

Exact working URLs:

```
https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=rhrread&lang=en
https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=fnd&lang=en
https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=flw&lang=en
https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=warnsum&lang=en
```

Confirmed properties:
- **HK regional only. No lat/lon, no location parameter, no API key.** It is
  published as government open data (data.gov.hk).
- **HTTPS** with HSTS; `Content-Type: application/json; charset=utf-8`.
- **CORS: `access-control-allow-origin: *`** (verified via response headers).
- **No documented rate limit or key requirement** in the API doc; treat as
  fair-use (cache responses; rhrread updates hourly, fnd twice daily, so
  polling more than ~ hourly gains nothing).
- Method: GET, return type JSON (API doc §1).

Sources:
- Open data catalogue + update frequencies: https://www.hko.gov.hk/en/abouthko/opendata_intro.htm
- Official API documentation (PDF, v1.13, Sep 2025): https://data.weather.gov.hk/weatherAPI/doc/HKO_Open_Data_API_Documentation.pdf
- Dataset listing (open data, JSON API resources): https://data.gov.hk/en-data/dataset/hk-hko-rss-current-weather-report
- Usage/readme: https://www.hko.gov.hk/en/readme/readme.htm

### 1.2 `rhrread` JSON structure (live sample 2026-11-19, truncated)

```json
{
  "temperature": {
    "data": [
      {"place": "King's Park", "value": 19, "unit": "C"},
      {"place": "Hong Kong Observatory", "value": 20, "unit": "C"},
      {"place": "Sha Tin", "value": 19, "unit": "C"},
      {"place": "Kwun Tong", "value": 20, "unit": "C"},
      {"place": "Kowloon City", "value": 19, "unit": "C"}
      /* ~27 stations total */
    ],
    "recordTime": "2026-11-19T12:00:00+08:00"
  },
  "humidity": {
    "data": [{"place": "Hong Kong Observatory", "value": 68, "unit": "percent"}],
    "recordTime": "2026-11-19T12:00:00+08:00"
  },
  "rainfall": {
    "data": [
      {"place": "Central & Western District", "max": 0, "unit": "mm", "main": "FALSE"},
      {"place": "Kwun Tong", "max": 0, "unit": "mm", "main": "FALSE"}
      /* 18 districts */
    ],
    "startTime": "2026-11-19T10:45:00+08:00",
    "endTime": "2026-11-19T11:45:00+08:00"
  },
  "icon": [60],
  "iconUpdateTime": "2026-11-19T08:10:00+08:00",
  "uvindex": {
    "data": [{"place": "King's Park", "value": 0.9, "desc": "low"}],
    "recordDesc": "During the past hour",
    "updateTime": "2026-11-19T12:02:00+08:00"
  },
  "warningMessage": "",
  "tcmessage": "",
  "mintempFrom00To09": "",
  "rainfallFrom00To12": "From midnight to noon, no rainfall was recorded at the Hong Kong Observatory.",
  "rainfallLastMonth": "",
  "rainfallJanuaryToLastMonth": ""
}
```

Fields to parse (API doc pp. 6–9):
- `temperature.data[]` → `{place, value, unit}` + `temperature.recordTime`
- `humidity.data[]` → `{place, value, unit}` + `humidity.recordTime`
- `rainfall.data[]` → `{place, max, unit, main}` + `startTime`/`endTime`
  (`main` = maintenance flag "TRUE"/"FALSE"; skip stations under maintenance)
- `icon[]` → HKO weather icon numbers (see §1.4); `iconUpdateTime`
- `uvindex.data[]` → `{place, value, desc}`; **may be an empty string when no data**
- `warningMessage` → warning text; **empty string when none; can be a list** —
  parser must accept string OR array OR missing
- There is **no single top-level `updateTime`** — each block carries its own
  `recordTime` / `iconUpdateTime` / `updateTime`. Use `temperature.recordTime`
  as the observation time.

**Parser robustness (documented footnotes):** "Parameter may be missing if the
value is null or not available"; UV index may be `""` instead of an object.
Optional blocks seen in the wild: `lightning`, `rainstormReminder`,
`specialWxTips`.

**District current temperature:** match `temperature.data[].place` against the
user's district. Station places include: Hong Kong Observatory, King's Park,
Wong Chuk Hang, Ta Kwu Ling, Lau Fau Shan, Tai Po, Sha Tin, Tuen Mun,
Tseung Kwan O, Sai Kung, Cheung Chau, Chek Lap Kok, Tsing Yi, Shek Kong,
Tsuen Wan Ho Koon, Tsuen Wan Shing Mun Valley, Hong Kong Park, Shau Kei Wan,
Kowloon City, Happy Valley, Wong Tai Sin, Stanley, Kwun Tong, Sham Shui Po,
Kai Tak Runway Park, Yuen Long Park, Tai Mei Tuk.
For a generic "Hong Kong" display use `place == "Hong Kong Observatory"`
(the HQ urban station). `rainfall.data[].place` uses the 18 district names
(Central & Western District, Eastern District, … Kwun Tong) — a different
naming scheme from temperature stations; map districts → nearest station.

### 1.3 `fnd` (9-day forecast) JSON structure (live sample, truncated)

```json
{
  "generalSituation": "Under the influence of an intense northeast monsoon ...",
  "weatherForecast": [
    {
      "forecastDate": "20261120",
      "week": "Friday",
      "forecastWind": "North to northeast force 4 to 5, occasionally force 6 offshore and on high ground.",
      "forecastWeather": "Temperatures will fall further. Cloudy and rather cool with a few rain patches.",
      "forecastMaxtemp": {"value": 18, "unit": "C"},
      "forecastMintemp": {"value": 15, "unit": "C"},
      "forecastMaxrh": {"value": 95, "unit": "percent"},
      "forecastMinrh": {"value": 75, "unit": "percent"},
      "ForecastIcon": 92,
      "PSR": "Low"
    }
    /* 9 entries */
  ],
  "updateTime": "2026-11-19T11:30:00+08:00",
  "seaTemp": {"place": "North Point", "value": 25, "unit": "C", "recordTime": "..."},
  "soilTemp": [ /* optional */ ]
}
```

Notes:
- Field is **`week`** (not `WeekDay`); `forecastDate` is a **`YYYYMMDD` string**.
- **`ForecastIcon` has a capital F** (inconsistent with `rhrread.icon`).
- `PSR` (Probability of Significant Rain): `High | Medium High | Medium |
  Medium Low | Low`.
- `flw` (local forecast) fields: `generalSituation`, `tcInfo`,
  `fireDangerWarning`, `forecastPeriod`, `forecastDesc`, `outlook`,
  `updateTime` — good for a "today/tonight" text panel.

### 1.4 HKO weather icon codes

`rhrread.icon[]` and `fnd.ForecastIcon` use HKO icon numbers. Official table:
https://www.hko.gov.hk/textonly/v2/explain/wxicon_e.htm

| Code | Meaning | Code | Meaning |
|------|---------|------|---------|
| 50 | Sunny | 70–75 | Fine (night, lunar-phase variants) |
| 51 | Sunny Periods | 76 | Mainly Cloudy (night) |
| 52 | Sunny Intervals | 77 | Mainly Fine (night) |
| 53 | Sunny Periods with A Few Showers | 80 | Windy |
| 54 | Sunny Intervals with Showers | 81 | Dry |
| 60 | Cloudy | 82 | Humid |
| 61 | Overcast | 83 | Fog |
| 62 | Light Rain | 84 | Mist |
| 63 | Rain | 85 | Haze |
| 64 | Heavy Rain | 90 | Hot |
| 65 | Thunderstorms | 91 | Warm |
|    |              | 92 | Cool |
|    |              | 93 | Cold |

Icon PNGs: `https://www.hko.gov.hk/images/HKOWxIconOutline/pic{N}.png`
(e.g. pic50.png … pic93.png).

### 1.5 `warnsum` (warning summary)

Returns `{}` when no warnings in force (**verified live**). Otherwise keyed by
warning code, e.g.:

```json
{"WRAIN": {"name": "Rainstorm Warning Signal", "code": "WRAINR",
           "actionCode": "ISSUE", "issueTime": "...", "updateTime": "..."}}
```

Codes: `WFIRE` (Y/R), `WFROST`, `WHOT`, `WCOLD`, `WMSGNL` (Strong Monsoon
Signal), `WRAIN` (WRAINA/R/B = Amber/Red/Black), `WFNTSA` (northern NT
flooding), `WL` (Landslip), `WTCSGNL` (TC1, TC3, TC8NE/SE/NW/SW, TC9, TC10,
CANCEL), `WTMW` (Tsunami), `WTS` (Thunderstorm). `actionCode`: ISSUE, REISSUE,
CANCEL, EXTEND, UPDATE. (API doc pp. 10–12.)

---

## 2. AccuWeather API — requires user-supplied API key

### 2.1 Free tier (verified from official Packages page)

- **Core Weather Free: 50 calls/day, limit 1 key per developer.**
- Free includes: Locations API, Current Conditions, 24h historical conditions,
  **5-day** daily forecast, **12-hour** hourly forecast, 5-day indices.
  (No alerts/translations on free.)
- Register at https://developer.accuweather.com → create account → "Get
  started now" free package → API key. (The portal blocks datacenter/bot IPs
  with HTTP 403; register from a normal browser.)
- **Branding requirement (Terms of Use):** AccuWeather logo, linked to
  http://www.accuweather.com, must appear on **every screen** showing
  AccuWeather data.
- **ToU restrictions relevant to KimiOS:** may not "commingle or supplement
  API Data … with any other weather information"; caching allowed up to
  2 weeks. → Keep AccuWeather as a distinct, user-selected provider; do not
  blend its data with Open-Meteo/HKO in the same view.

Source (official page, archived copy — live portal is bot-gated):
https://developer.accuweather.com/packages
(archive: http://web.archive.org/web/20250908115745/https://developer.accuweather.com/packages)

### 2.2 Flow: lat/lon → Location Key → Current Conditions

**Step 1 — Geoposition Search (Locations API):**

```
GET https://dataservice.accuweather.com/locations/v1/cities/geoposition/search?apikey=KEY&q=22.3027,114.1772
```

| Param | Required | Notes |
|-------|----------|-------|
| `apikey` | yes | developer key |
| `q` | yes | **`lat,lon`** comma-separated pair |
| `language` | no | e.g. `en-us` |
| `details` | no | `true` for full object |
| `toplevel` | no | `true` → highest-rank city (e.g. "New York" instead of "Greenwich Village") |

Response (single object) — key fields: **`Key`** (the location key string),
`LocalizedName`, `EnglishName`, `Country.ID` (e.g. "HK"), `Country.EnglishName`,
`AdministrativeArea.ID`, `Region.ID`, `GeoPosition.Latitude`,
`GeoPosition.Longitude`, `TimeZone.Name`, `Rank`, `Type`.
**Cache `Key` per location** — it is stable; this saves half your quota.

**Step 2 — Current Conditions:**

```
GET https://dataservice.accuweather.com/currentconditions/v1/{locationKey}?apikey=KEY&details=true&language=en-us
```

- `details=true` returns the full object (default is truncated).
- **Response is a JSON ARRAY with one element** — parse `[0]`.

Key response fields (official API reference):

```
[0].LocalObservationDateTime   ISO8601 string
[0].EpochTime                  int64 seconds
[0].WeatherText                "Cloudy"
[0].WeatherIcon                int (AccuWeather icon number, may be null)
[0].IsDayTime                  bool
[0].Temperature.Metric.Value   double (°C)   .Unit "C"  .UnitType int
[0].Temperature.Imperial.Value double (°F)
[0].RealFeelTemperature.Metric.Value
[0].RelativeHumidity           int (%)
[0].Wind.Direction.Degrees     int
[0].Wind.Direction.Localized   "NNE"
[0].Wind.Speed.Metric.Value    double (km/h)
[0].WindGust.Speed.Metric.Value
[0].UVIndex                    int
[0].Visibility.Metric.Value
[0].CloudCover                 int (%)
[0].Pressure.Metric.Value
[0].HasPrecipitation           bool
[0].PrecipitationType          "Rain"|"Snow"|"Ice"|"Mixed"|null
[0].MobileLink / [0].Link      URLs for attribution
```

Error codes: 400 bad params, **401 invalid/missing API key**, 403 no
permission, 404 unknown route, 500 server error.

**HTTPS: confirmed working.** Docs list the resource URL as
`http://dataservice.accuweather.com/...` but the same paths answer on TLS:
live test `https://dataservice.accuweather.com/currentconditions/v1/123?apikey=test`
→ HTTP 401 (auth error, not TLS error). **Use https:// in KimiOS.**

Sources (official API reference pages, archived copies):
- Current Conditions: https://developer.accuweather.com/accuweather-current-conditions-api/apis/get/currentconditions/v1/%7BlocationKey%7D
  (archive: http://web.archive.org/web/20250717052034/https://developer.accuweather.com/accuweather-current-conditions-api/apis/get/currentconditions/v1/%7BlocationKey%7D)
- Geoposition Search: https://developer.accuweather.com/accuweather-locations-api/apis/get/locations/v1/cities/geoposition/search
  (archive: http://web.archive.org/web/20250124212846/https://developer.accuweather.com/accuweather-locations-api/apis/get/locations/v1/cities/geoposition/search)

### 2.3 Feasibility as no-key default: NOT feasible

Key is mandatory (401 without). 50 calls/day ≈ 25 refreshes/day if the
location key is re-resolved each time, or ~50 with a cached key. Fine as an
**optional provider behind a user-entered key** in settings; unsuitable as
the out-of-the-box default.

---

## 3. Open-Meteo — keep as global fallback (modernize the params)

### 3.1 Modern `current=` endpoint (verified live)

```
GET https://api.open-meteo.com/v1/forecast
    ?latitude=22.3027&longitude=114.1772
    &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,weather_code,cloud_cover,wind_speed_10m,wind_direction_10m
    &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max
    &timezone=auto&forecast_days=7
```

Live response (2026-11-19, current-only request):

```json
{
  "latitude": 22.319859, "longitude": 114.198555,
  "utc_offset_seconds": 28800, "timezone": "Asia/Hong_Kong",
  "elevation": 16.0,
  "current_units": {"time":"iso8601","interval":"seconds","temperature_2m":"°C",
                    "relative_humidity_2m":"%","apparent_temperature":"°C",
                    "weather_code":"wmo code","wind_speed_10m":"km/h"},
  "current": {"time":"2026-11-19T12:30","interval":900,
              "temperature_2m":21.1,"relative_humidity_2m":58,
              "apparent_temperature":20.0,"weather_code":3,"wind_speed_10m":12.5}
}
```

- `current_weather=true` (what KimiOS uses today) **still works** (verified
  live: returns `current_weather.{temperature,windspeed,winddirection,weathercode,is_day,time}`)
  but it is the legacy interface; the documented modern parameter is
  **`current=`** with an explicit variable list, which adds humidity,
  apparent temperature, precipitation, cloud cover.
- `latitude`/`longitude` required (WGS84 floats). `timezone=auto` resolves
  local time; **`timezone` is required whenever `daily=` is requested**.
- Units default: °C, km/h, mm. Options: `temperature_unit=fahrenheit`,
  `wind_speed_unit=ms|mph|kn`, `timeformat=unixtime`.
- Free, **no API key**, global coverage. `apikey` param exists **only for
  commercial use** (customer- endpoint prefix). Non-commercial use is free.

Source: https://open-meteo.com/en/docs (params table, current-weather section,
WMO table); pricing/licence: https://open-meteo.com/en/pricing

### 3.2 WMO weather_code table (complete, from official docs)

| Code | Meaning | Code | Meaning |
|------|---------|------|---------|
| 0 | Clear sky | 61 | Slight rain |
| 1 | Mainly clear | 63 | Moderate rain |
| 2 | Partly cloudy | 65 | Heavy rain |
| 3 | Overcast | 66 | Light freezing rain |
| 45 | Fog | 67 | Heavy freezing rain |
| 48 | Depositing rime fog | 71 | Slight snowfall |
| 51 | Light drizzle | 73 | Moderate snowfall |
| 53 | Moderate drizzle | 75 | Heavy snowfall |
| 55 | Dense drizzle | 77 | Snow grains |
| 56 | Light freezing drizzle | 80 | Slight rain showers |
| 57 | Dense freezing drizzle | 81 | Moderate rain showers |
|    |                        | 82 | Violent rain showers |
| 85 | Slight snow showers | 95 | Thunderstorm |
| 86 | Heavy snow showers | 96 | Thunderstorm with slight hail* |
|    |                      | 97 | Heavy thunderstorm |
|    |                      | 99 | Thunderstorm with heavy hail* |

\* 96/99 only from models with explicit hail forecasts (DWD ICON, UKMO);
other models report 95/97. Grouping for UI: 0 clear; 1–3 partly cloudy/overcast;
45/48 fog; 51–57 drizzle; 61–67 rain; 71–77 snow; 80–82 showers; 85–86 snow
showers; 95–99 thunderstorm. The API returns numbers only — map client-side
(the docs publish a ready-to-copy JSON mapping).

---

## 4. Android location reliability on a virtual device

### 4.1 Why `getLastKnownLocation` returns null

Platform API contract: "Gets the last known location from the given provider,
**or null if there is no last known location**." It is a **cache read only —
it never activates GPS or computes a fix** (contrast with `getCurrentLocation`,
API 30+, which "may activate sensors in order to compute a new location").

On Andarium (Android 16 virtual device, targetSdk 28) the null causes are:
1. **No prior fix ever recorded** — a fresh virtual device has an empty
   location cache; nothing has ever requested a fix.
2. **No GNSS hardware / no injected fix** — emulator GPS only produces a fix
   after the host sends one (`geo fix`, §4.4). If Andarium doesn't expose
   that, `gps` never fixes.
3. **`network` provider may not exist** — network location typically depends
   on Google Play services / a network location provider; a bare virtual
   device may lack it. Check `isProviderEnabled()` / `getAllProviders()`.
4. **Location master switch off** — providers disabled; cache may be cleared.
5. **Permission not granted** — with targetSdk 28 on an Android 6+ device,
   location is a *dangerous* permission requiring a **runtime grant**;
   without it the call throws `SecurityException` (or returns nothing if the
   app swallows it). Manifest declaration alone is not enough.
6. `PASSIVE_PROVIDER` only piggybacks on fixes computed for *other* apps —
   it cannot originate a fix on an idle device.

Sources:
- https://developer.android.com/reference/android/location/LocationManager
  (getLastKnownLocation, getCurrentLocation, requestLocationUpdates,
  GPS_PROVIDER/NETWORK_PROVIDER/PASSIVE_PROVIDER)
- https://developer.android.com/training/permissions/requesting
  (runtime permission workflow; API 23+ devices require runtime requests)
- https://developer.android.com/develop/sensors-and-location/location/permissions
- https://developer.android.com/develop/sensors-and-location/location/retrieve-current
  (last-location null cases; note: this page covers the Play-services fused
  provider — KimiOS uses platform LocationManager, which has no fused client
  unless GMS is present)

### 4.2 Permissions + runtime grant (targetSdk 28 pattern)

Manifest:

```xml
<uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION"/>
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"/>
```

Runtime flow (works with plain framework APIs, no AndroidX needed):

```java
if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION)
        != PackageManager.PERMISSION_GRANTED) {
    requestPermissions(new String[]{
        Manifest.permission.ACCESS_FINE_LOCATION,
        Manifest.permission.ACCESS_COARSE_LOCATION}, REQ_LOCATION);
    return; // continue in onRequestPermissionsResult
}
// granted → proceed to location
```

- On Android 12+ (device is 16) the dialog offers **Precise / Approximate**;
  handle a coarse-only grant gracefully (coarse ≈ 3 km accuracy is plenty
  for weather).
- If denied: **do not block** — fall back to IP geolocation (§4.5) and/or a
  manual city picker. Test installs can auto-grant: `adb install -g app.apk`
  or `adb shell pm grant <pkg> android.permission.ACCESS_FINE_LOCATION`.

### 4.3 Active fix: `requestLocationUpdates` pattern

```java
LocationManager lm = (LocationManager) getSystemService(LOCATION_SERVICE);
Location best = null;
for (String p : new String[]{LocationManager.GPS_PROVIDER,
                             LocationManager.NETWORK_PROVIDER,
                             LocationManager.PASSIVE_PROVIDER}) {
    if (lm.isProviderEnabled(p)) {
        Location l = lm.getLastKnownLocation(p);   // cache read, may be null
        if (l != null && (best == null || l.getTime() > best.getTime())) best = l;
    }
}
if (best == null || stale(best)) {
    LocationListener listener = new LocationListener() {
        @Override public void onLocationChanged(Location loc) {
            // first fix → use loc, then lm.removeUpdates(this)
        }
    };
    for (String p : new String[]{LocationManager.GPS_PROVIDER,
                                 LocationManager.NETWORK_PROVIDER}) {
        if (lm.isProviderEnabled(p)) {
            lm.requestLocationUpdates(p, 0L, 0f, listener, Looper.getMainLooper());
        }
    }
    // post a timeout (~10–15 s): removeUpdates(listener) and fall back to IP geo
}
```

API notes (LocationManager reference):
- `requestLocationUpdates(String provider, long minTimeMs, float minDistanceM,
  LocationListener listener[, Looper])` — callbacks on the given looper;
  unregister with `removeUpdates(listener)`.
- "It may take some time to receive the first location update" — the docs
  explicitly suggest cached locations or `getCurrentLocation` instead.
- On API 30+ devices, `lm.getCurrentLocation(provider, cancellationSignal,
  executor, consumer)` is the cleaner one-shot active fix (guaranteed fresh,
  single callback incl. null on failure) — available on the Andarium device
  (API 37) if compileSdk permits; guard with `Build.VERSION.SDK_INT >= 30`.
- Never leave updates registered after the fix/timeout (battery).

### 4.4 Injecting a GPS fix on emulators / virtual devices

Standard Android emulator console (documented):

```
adb emu geo fix <longitude> <latitude> [altitude] [satellites] [velocity]
# or: telnet localhost 5554 → auth <token> → geo fix 114.1772 22.3027
```

- **Longitude comes FIRST.** The emulator creates a mock location provider
  that feeds `LocationManager`; apps can then read it via
  `getLastKnownLocation("gps")` or listeners.
- This only works if Andarium exposes the emulator console / `adb emu`
  channel. If it doesn't (likely for a custom virtual device), **assume GPS
  will never fix** and rely on the IP fallback. Also check Andarium settings
  for a location/mock-location toggle.

Source: https://developer.android.com/studio/run/emulator-console
("Geographic location" / `geo fix`; `adb emu` fire-and-forget)

### 4.5 IP geolocation fallback (no GPS, no permission, works anywhere with internet)

| Service | Endpoint | TLS | Key | Free limit | Coord fields |
|---------|----------|-----|-----|------------|--------------|
| **ipwho.is (recommended)** | `GET https://ipwho.is/` | **HTTPS free** | none | 1,000 req/day per client IP; commercial use allowed | `latitude`, `longitude` |
| ipapi.co (secondary) | `GET https://ipapi.co/json/` | HTTPS free | none | rate-limited free tier (429 + `{error:true,reason:"RateLimited"}`) | `latitude`, `longitude`, `latlong` |
| ip-api.com | `GET http://ip-api.com/json/?fields=status,country,countryCode,city,lat,lon` | **HTTP only on free tier** (HTTPS = paid) | none | 45 req/min per IP; **non-commercial only** | **`lat`, `lon`** (different names!) |

**ipwho.is — verified live from this environment (2026-11-19):**

```json
{
  "ip": "182.239.89.202", "success": true,
  "country": "Hong Kong", "country_code": "HK",
  "city": "Hong Kong", "region": "Hong Kong",
  "latitude": 22.2783168, "longitude": 114.1746891,
  "timezone": {"id": "Asia/Hong_Kong", "offset": 28800, "utc": "+08:00"},
  "connection": {"isp": "China Mobile Hong Kong Company Limited", "asn": 9231}
}
```

Parse: `success == true` → `latitude`, `longitude`, `city`, `country_code`.
Failure modes: HTTP 429 (quota, `Retry-After` header) or HTTP 200 with
`{"success":false,"message":"..."}` (e.g. reserved IP range). Free tier omits
`security`/`currency` blocks — don't depend on them.
Docs: https://ipwhois.io/documentation

**ipapi.co** — `https://ipapi.co/json/` returns `latitude`, `longitude`,
`city`, `country_name`, `country_code`, `timezone`, `utc_offset`, `latlong`
("lat,lon" string). Docs: https://ipapi.co/api/
**Observed caveat:** from this datacenter IP, ipapi.co answered with a
Cloudflare browser challenge ("Just a moment…") instead of JSON — it can
reject non-browser clients. Keep it as a *secondary* fallback only.

**ip-api.com** — free endpoint is HTTP-only ("256-bit SSL encryption is not
available for this free API"), 45 req/min, non-commercial only; fields are
`lat`/`lon`/`city`/`country`/`countryCode`/`status`. Only usable if KimiOS
adds a plain-HTTP syscall path; otherwise skip.
Docs: https://ip-api.com/docs/api:json

### 4.6 Recommended layered location strategy

1. **Runtime permission** (FINE+COARSE) — request once, in context.
2. If granted: **last-known across gps/network/passive**, newest by
   `Location.getTime()`; accept if reasonably fresh (e.g. < 30 min).
3. Else **active fix**: `requestLocationUpdates` on enabled gps+network (or
   `getCurrentLocation` on API 30+), **hard timeout ~10–15 s**, then
   `removeUpdates`.
4. If no fix / permission denied / providers absent: **IP geolocation**
   `https://ipwho.is/` (secondary `https://ipapi.co/json/`). No permission
   needed; city-level accuracy is sufficient for weather.
5. **Cache** last good lat/lon + city + provider choice on disk; reuse when
   offline. Offer a **manual city override** as the final fallback.
6. **Provider routing:** if `country_code == "HK"` (or coords inside HK
   bounding box ≈ lat 22.15–22.60, lon 113.80–114.45) → **HKO**; else →
   **Open-Meteo** with the resolved lat/lon.

---

## 5. RECOMMENDATION (drives implementation)

**Weather providers:**
1. **Default = HKO for HK + Open-Meteo for everywhere else.** Both are free,
   no-key, HTTPS GET + JSON — they fit the existing `HttpURLConnection`
   syscall and no_std Rust parser with zero new infrastructure. HKO gives
   official HK district temperatures, humidity, rainfall, UV, warnings
   (`warnsum`) and a 9-day forecast (`fnd`); Open-Meteo `current=` +
   `daily=` covers the rest of the world. Migrate the legacy
   `current_weather=true` call to `current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m`
   (keep WMO code mapping client-side).
2. **AccuWeather = optional, behind a user-supplied API key** (settings
   field). Cache the location key from geoposition search; cache conditions
   (≤ 2 weeks per ToU); display the linked AccuWeather logo on every screen
   showing its data; keep it a separate provider (ToU forbids commingling
   with other weather data). 50 calls/day makes it unusable as a default.

**Location:**
3. Never rely on `getLastKnownLocation` alone on Andarium. Implement
   §4.6: permission → last-known → active updates with timeout →
   **ipwho.is HTTPS IP-geo fallback** → cached/manual city. This makes the
   app work on any virtual device with internet, even with no GPS, no GMS,
   and no location permission.

**Parser gotchas to handle in no_std Rust:**
- HKO: optional/missing keys; `uvindex` may be `""`; `warningMessage` string
  OR array; `fnd.forecastDate` = `YYYYMMDD` string; `ForecastIcon` capital F;
  `rainfall.data[].main` maintenance flag; per-block timestamps (no global
  `updateTime` in rhrread).
- Open-Meteo: values under `current.*`, units under `current_units.*`;
  `weather_code` numeric WMO.
- AccuWeather: **root is an array**; `Temperature.Metric.Value` may be null;
  401 → prompt for key.
- ipwho.is: check `success` boolean even on HTTP 200.

---

## Sources (all accessed 2026-11-19)

HKO:
- https://www.hko.gov.hk/en/abouthko/opendata_intro.htm
- https://data.weather.gov.hk/weatherAPI/doc/HKO_Open_Data_API_Documentation.pdf (v1.13)
- https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=rhrread&lang=en (live)
- https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=fnd&lang=en (live)
- https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=flw&lang=en (live)
- https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=warnsum&lang=en (live)
- https://www.hko.gov.hk/textonly/v2/explain/wxicon_e.htm
- https://data.gov.hk/en-data/dataset/hk-hko-rss-current-weather-report

AccuWeather (official pages; portal bot-gated, used Wayback captures):
- https://developer.accuweather.com/packages → http://web.archive.org/web/20250908115745/https://developer.accuweather.com/packages
- https://developer.accuweather.com/accuweather-current-conditions-api/apis/get/currentconditions/v1/%7BlocationKey%7D → http://web.archive.org/web/20250717052034/https://developer.accuweather.com/accuweather-current-conditions-api/apis/get/currentconditions/v1/%7BlocationKey%7D
- https://developer.accuweather.com/accuweather-locations-api/apis/get/locations/v1/cities/geoposition/search → http://web.archive.org/web/20250124212846/https://developer.accuweather.com/accuweather-locations-api/apis/get/locations/v1/cities/geoposition/search
- https://dataservice.accuweather.com/currentconditions/v1/123?apikey=test (live HTTPS probe → 401)

Open-Meteo:
- https://open-meteo.com/en/docs (params, current weather, WMO table)
- https://open-meteo.com/en/pricing
- https://api.open-meteo.com/v1/forecast?...&current=... (live)
- https://api.open-meteo.com/v1/forecast?...&current_weather=true (live, legacy)

Android:
- https://developer.android.com/reference/android/location/LocationManager
- https://developer.android.com/training/permissions/requesting
- https://developer.android.com/develop/sensors-and-location/location/permissions
- https://developer.android.com/develop/sensors-and-location/location/retrieve-current
- https://developer.android.com/develop/sensors-and-location/location/request-updates
- https://developer.android.com/studio/run/emulator-console (geo fix)

IP geolocation:
- https://ipwhois.io/documentation (+ live https://ipwho.is/)
- https://ipapi.co/api/ (live probe hit Cloudflare challenge from datacenter IP)
- https://ip-api.com/docs/api:json

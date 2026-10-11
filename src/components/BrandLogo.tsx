import wechatLogo from "../assets/wechat.svg";
import dingtalkLogo from "../assets/dingtalk.png";

export type BrandLogoName = "wechat" | "dingtalk";

const BRAND_LOGO_SOURCES: Record<BrandLogoName, string> = {
  wechat: wechatLogo,
  dingtalk: dingtalkLogo,
};

/**
 * 本地品牌资源只作为设置页的识别图形使用，不依赖运行时 CDN。
 * 微信是官方标志的矢量路径数据（品牌绿），钉钉是品牌蓝标志位图；
 * 两者都是商标资源，仅作品牌识别，不改形、不改色。
 * alt 由相邻文字提供，避免同一个品牌名称在读屏器中重复播报。
 */
export function BrandLogo({ brand }: { brand: BrandLogoName }) {
  return (
    <img
      className="mobile-settings__brand-logo"
      data-brand-logo={brand}
      src={BRAND_LOGO_SOURCES[brand]}
      alt=""
      aria-hidden="true"
    />
  );
}

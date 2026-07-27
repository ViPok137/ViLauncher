const GITHUB_OWNER = 'ViPok137';
const GITHUB_REPO = 'ViLauncher';

// 1. Авто-получение последней версии .exe с GitHub Releases
async function fetchLatestRelease() {
    const btn = document.getElementById('download-btn');
    const versionTag = document.getElementById('version-tag');

    try {
        const response = await fetch(`https://api.github.com/repos/${GITHUB_OWNER}/${GITHUB_REPO}/releases/latest`);
        if (!response.ok) throw new Error('Не удалось загрузить релиз');
        
        const data = await response.json();
        const exeAsset = data.assets.find(asset => asset.name.endsWith('.exe'));

        if (exeAsset) {
            btn.href = exeAsset.browser_download_url;
            btn.textContent = '🚀 Скачать ViLauncher (.exe)';
            versionTag.textContent = `Версия: ${data.tag_name} (${(exeAsset.size / (1024 * 1024)).toFixed(1)} MB)`;
        } else {
            btn.textContent = 'EXE файл не найден';
        }
    } catch (err) {
        btn.textContent = 'Перейти к релизу';
        btn.href = `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}/releases`;
        console.error(err);
    }
}

// 2. Получение новостей из config/News.json вашего репозитория
async function fetchNews() {
    const newsContainer = document.getElementById('news-container');
    const newsUrl = `https://raw.githubusercontent.com/${GITHUB_OWNER}/${GITHUB_REPO}/main/config/News.json`;

    try {
        const response = await fetch(newsUrl);
        if (!response.ok) throw new Error('Ошибка загрузки новостей');

        const news = await response.json();
        newsContainer.innerHTML = '';

        // Выводим новости в обратном порядке (новые сверху)
        news.reverse().forEach(item => {
            const card = document.createElement('div');
            card.className = 'news-card';
            card.innerHTML = `
                <h3>${item.title}</h3>
                <div class="news-date">📅 ${item.date}</div>
                <p>${item.body}</p>
            `;
            newsContainer.appendChild(card);
        });
    } catch (err) {
        newsContainer.innerHTML = '<p>Не удалось загрузить новости.</p>';
        console.error(err);
    }
}

// Запуск при загрузке страницы
document.addEventListener('DOMContentLoaded', () => {
    fetchLatestRelease();
    fetchNews();
});
